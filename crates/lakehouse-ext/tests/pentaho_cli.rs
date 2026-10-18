//! RFC 0204 through the real command: `lakehouse pentaho translate` proposes a branch, `--out`
//! writes into a directory whose Pentaho files `generate` then leaves alone, and the report reads
//! the proposal back.

use ekos::extension::Extensions;
use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
use ekos_lakehouse_bronze::{SourceColumn, SourceTable, TypeKind};
use ekos_lakehouse_ext::LakehouseExtension;
use ekos_lakehouse_ext::pentaho_cmd::load_report;
use ekos_lakehouse_ext::plan_cmd::write;
use ekos_lakehouse_silver::SilverSettings;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

fn col(n: &str, pg: &str, trino: &str) -> SourceColumn {
    SourceColumn {
        name: n.into(),
        pg_type: pg.into(),
        kind: TypeKind::Base,
        not_null: false,
        trino_type: Some(trino.into()),
        numeric_used: None,
        comment: None,
        enum_labels: vec![],
        pii_candidate: None,
        infinite_values: None,
        attnum: 0,
        has_default: false,
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const KTR: &str = r#"<transformation><info><name>orders</name></info>
  <connection><name>src</name><type>POSTGRESQL</type><database>shop</database><password>Encrypted 2be98afc86aa7b88bb81cbe788fc0fce5</password></connection>
  <connection><name>dwh</name><type>POSTGRESQL</type><database>dwh</database></connection>
  <order><hop><from>in</from><to>calc</to><enabled>Y</enabled></hop><hop><from>calc</from><to>out</to><enabled>Y</enabled></hop></order>
  <step><name>in</name><type>TableInput</type><connection>src</connection><sql>SELECT id, qty, price FROM orders</sql></step>
  <step><name>calc</name><type>Calculator</type><calculation><field_name>amount</field_name><calc_type>MULTIPLY</calc_type><field_a>qty</field_a><field_b>price</field_b><value_type>BigNumber</value_type></calculation></step>
  <step><name>out</name><type>TableOutput</type><connection>dwh</connection><table>order_amounts</table><truncate>Y</truncate></step>
</transformation>"#;

#[tokio::test(flavor = "current_thread")]
async fn translate_proposes_a_branch_and_generate_keeps_the_files() {
    let root = std::env::temp_dir().join(format!("ekos-lh-pentaho-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("etl")).unwrap();
    std::fs::write(root.join("etl/orders.ktr"), KTR).unwrap();
    std::fs::write(
        root.join("ekos.toml"),
        "[workspace]\nname = \"t\"\n\n[[sources]]\nname = \"etl\"\npath = \"etl\"\n\n\
         [extensions.lakehouse]\nsource-database = \"shop\"\npipeline-dir = \"pipeline\"\n",
    )
    .unwrap();
    let p = plan(
        &[SourceTable {
            schema: "public".into(),
            oid: 0,
            name: "orders".into(),
            columns: vec![
                col("id", "integer", "integer"),
                col("qty", "integer", "integer"),
                col("price", "numeric(10,2)", "decimal(10,2)"),
            ],
            primary_key: vec!["id".into()],
            row_estimate: 1,
            update_triggers: vec![],
            foreign_keys: vec![],
            comment: None,
        }],
        &PlanSettings::default(),
    );
    let repo = root.join("pipeline");
    std::fs::create_dir_all(&repo).unwrap();
    write(&p, &SilverSettings::default(), &repo).unwrap();
    std::fs::write(
        repo.join("lakehouse.lock.json"),
        serde_json::to_string_pretty(&p).unwrap(),
    )
    .unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "generate"]);

    std::env::set_current_dir(&root).unwrap();
    let ext = Extensions::new(vec![Arc::new(LakehouseExtension)]);
    ekos::app::main_with_args(
        ext.clone(),
        [
            "ekos-lakehouse",
            "lakehouse",
            "pentaho",
            "translate",
            "etl",
            "--no-explain",
        ],
    )
    .await
    .unwrap();
    let branches = git(&repo, &["branch", "--list", "ekos/pentaho/etl/*"]);
    assert_eq!(branches.lines().count(), 1, "{branches}");
    let branch = branches.trim().trim_start_matches("* ").to_string();
    let gold = git(
        &repo,
        &[
            "show",
            &format!("{branch}:dbt/models/gold/pentaho/etl/gold__order_amounts.sql"),
        ],
    );
    assert!(gold.contains("ref('pentaho__etl__orders__in')"), "{gold}");
    let json = git(
        &repo,
        &["show", &format!("{branch}:pentaho/etl/translation.json")],
    );
    assert!(
        !json.contains("2be98afc"),
        "the password never reaches the repository"
    );
    let (report, state) = load_report(&repo, "etl").expect("a report");
    assert!(
        state.starts_with("proposed on ekos/pentaho/etl/"),
        "{state}"
    );
    assert!(
        report.models.iter().all(|m| m.enabled),
        "{:#?}",
        report.models
    );
    // `main_with_args` installs the global logger, so the later runs call the command itself.
    let config = ekos_compiler_core::EkosConfig::from_file(&root.join("ekos.toml")).unwrap();
    let settings = ekos_lakehouse_ext::settings::load(&config).unwrap();
    let ctx = ekos::extension::CommandContext {
        config: &config,
        cwd: &root,
        config_path: &root.join("ekos.toml"),
        invoked_from: &root,
        extensions: &ext,
    };
    let run = |out: Option<std::path::PathBuf>| ekos_lakehouse_ext::pentaho_cmd::TranslateOptions {
        package: "etl".into(),
        out,
        explain: false,
        json: false,
        allow_metered: false,
    };
    // Running it again proposes nothing new.
    ekos_lakehouse_ext::pentaho_cmd::translate(&settings, &ctx, &run(None), &repo)
        .await
        .unwrap();
    assert_eq!(
        git(&repo, &["branch", "--list", "ekos/pentaho/etl/*"])
            .lines()
            .count(),
        1
    );

    // --out into the working tree, then `generate` again: the Pentaho files stay.
    ekos_lakehouse_ext::pentaho_cmd::translate(&settings, &ctx, &run(Some(repo.clone())), &repo)
        .await
        .unwrap();
    assert!(
        repo.join("dbt/models/gold/pentaho/etl/gold__order_amounts.sql")
            .exists()
    );
    let w = write(&p, &SilverSettings::default(), &repo).unwrap();
    assert!(w.removed.is_empty(), "generate removed {:?}", w.removed);
    assert!(
        repo.join("dbt/models/gold/pentaho/etl/gold__order_amounts.sql")
            .exists()
    );
    std::env::set_current_dir(std::env::temp_dir()).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

struct FakeDecompiler;

impl ekos::extension::JavaDecompiler for FakeDecompiler {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn decompile(
        &self,
        archive: &[u8],
        class: &str,
    ) -> anyhow::Result<ekos::extension::DecompiledClass> {
        anyhow::ensure!(archive == b"PK-fake-jar", "not the plugin's jar");
        anyhow::ensure!(class == "com/acme/Doubler", "no class {class}");
        Ok(ekos::extension::DecompiledClass {
            class: class.into(),
            super_class: Some("org/pentaho/di/trans/step/BaseStep".into()),
            interfaces: vec![],
            methods: vec![ekos::extension::DecompiledMethod {
                name: "processRow".into(),
                descriptor: "(Lorg/pentaho/di/trans/step/StepMetaInterface;Lorg/pentaho/di/trans/step/StepDataInterface;)Z".into(),
                fidelity: ekos::extension::DecompiledFidelity::Statements,
                source: "Object[] r = getRow();\nif (r == null) { setOutputDone(); return false; }\nLong qty = get(Fields.In, \"qty\").getInteger(r);\nget(Fields.Out, \"qty\").setValue(r, qty * 2);\nputRow(data.outputRowMeta, r);\nreturn true;".into(),
                gaps: vec![],
            }],
            diagnostics: vec![],
        })
    }
}

struct Decompiles;

#[async_trait::async_trait(?Send)]
impl ekos::extension::EkosExtension for Decompiles {
    fn name(&self) -> &'static str {
        "fake-binary"
    }

    fn java_decompiler(&self) -> Option<Arc<dyn ekos::extension::JavaDecompiler>> {
        Some(Arc::new(FakeDecompiler))
    }
}

/// RFC 0203: a compiled plugin step is decompiled through the seam when the binary has a
/// decompiler, and stays a gap that says why when it does not.
#[tokio::test(flavor = "current_thread")]
async fn plugin_steps_go_through_the_java_decompiler_seam() {
    let root = std::env::temp_dir().join(format!("ekos-lh-pentaho-plugin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("etl/plugins/doubler")).unwrap();
    std::fs::write(
        root.join("etl/plugins/doubler/plugin.xml"),
        r#"<plugin id="AcmeDoubler" classname="com.acme.DoublerMeta"/>"#,
    )
    .unwrap();
    std::fs::write(root.join("etl/plugins/doubler/doubler.jar"), b"PK-fake-jar").unwrap();
    let ktr = KTR
        .replace("<type>Calculator</type><calculation><field_name>amount</field_name><calc_type>MULTIPLY</calc_type><field_a>qty</field_a><field_b>price</field_b><value_type>BigNumber</value_type></calculation>", "<type>AcmeDoubler</type>");
    std::fs::write(root.join("etl/orders.ktr"), ktr).unwrap();
    std::fs::write(
        root.join("ekos.toml"),
        "[workspace]\nname = \"t\"\n\n[[sources]]\nname = \"etl\"\npath = \"etl\"\n\n[extensions.lakehouse]\nsource-database = \"shop\"\n",
    )
    .unwrap();
    let p = plan(
        &[SourceTable {
            schema: "public".into(),
            oid: 0,
            name: "orders".into(),
            columns: vec![
                col("id", "integer", "integer"),
                col("qty", "integer", "integer"),
                col("price", "numeric(10,2)", "decimal(10,2)"),
            ],
            primary_key: vec!["id".into()],
            row_estimate: 1,
            update_triggers: vec![],
            foreign_keys: vec![],
            comment: None,
        }],
        &PlanSettings::default(),
    );
    let repo = root.join("pipeline");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("lakehouse.lock.json"),
        serde_json::to_string_pretty(&p).unwrap(),
    )
    .unwrap();
    let config = ekos_compiler_core::EkosConfig::from_file(&root.join("ekos.toml")).unwrap();
    let settings = ekos_lakehouse_ext::settings::load(&config).unwrap();
    let step = |out: &Path| -> serde_json::Value {
        let r: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(out.join("pentaho/etl/translation.json")).unwrap(),
        )
        .unwrap();
        r["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["step"] == "calc")
            .unwrap()
            .clone()
    };
    // Public build: no decompiler.
    let public = root.join("out-public");
    translate_into(
        &config,
        &settings,
        &root,
        &repo,
        &Extensions::new(vec![Arc::new(LakehouseExtension)]),
        &public,
    )
    .await;
    let s = step(&public);
    assert_eq!(s["fidelity"], "gap");
    assert!(
        s["gap"]
            .as_str()
            .unwrap()
            .contains("needs the ekos-binary build"),
        "{s}"
    );
    // With a decompiler.
    let private = root.join("out-private");
    translate_into(
        &config,
        &settings,
        &root,
        &repo,
        &Extensions::new(vec![Arc::new(LakehouseExtension), Arc::new(Decompiles)]),
        &private,
    )
    .await;
    let s = step(&private);
    assert_eq!(s["fidelity"], "rewritten", "{s}");
    assert!(
        s["original"].as_str().unwrap().starts_with("// decompiled"),
        "{s}"
    );
    assert!(s["sql"].as_str().unwrap().contains("(\"qty\" * 2)"), "{s}");
    let _ = std::fs::remove_dir_all(&root);
}

async fn translate_into(
    config: &ekos_compiler_core::EkosConfig,
    settings: &ekos_lakehouse_ext::settings::Settings,
    root: &Path,
    repo: &Path,
    ext: &Extensions,
    out: &Path,
) {
    let config_path = root.join("ekos.toml");
    let ctx = ekos::extension::CommandContext {
        config,
        cwd: root,
        config_path: &config_path,
        invoked_from: root,
        extensions: ext,
    };
    let o = ekos_lakehouse_ext::pentaho_cmd::TranslateOptions {
        package: "etl".into(),
        out: Some(out.to_path_buf()),
        explain: false,
        json: false,
        allow_metered: false,
    };
    ekos_lakehouse_ext::pentaho_cmd::translate(settings, &ctx, &o, repo)
        .await
        .unwrap();
}

/// Answers in turn from a script, recording each prompt.
struct Scripted {
    answers: std::sync::Mutex<Vec<String>>,
    prompts: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ekos_recovery::LlmProvider for Scripted {
    fn model_name(&self) -> &str {
        "scripted"
    }

    async fn complete(
        &self,
        req: &ekos_recovery::LlmRequest<'_>,
    ) -> Result<ekos_recovery::LlmResponse, ekos_recovery::LlmError> {
        self.prompts.lock().unwrap().push(req.user.to_string());
        let content = self.answers.lock().unwrap().remove(0);
        Ok(ekos_recovery::LlmResponse {
            content,
            model: "scripted".into(),
            ..Default::default()
        })
    }
}

/// RFC 0205: a gap code step is drafted, refused answers are fed back, and the survivor is
/// llm-drafted in the translation.
#[tokio::test(flavor = "current_thread")]
async fn a_model_drafts_a_gap_code_step_and_only_checked_sql_survives() {
    let ktr = KTR.replace(
        "<type>Calculator</type><calculation><field_name>amount</field_name><calc_type>MULTIPLY</calc_type><field_a>qty</field_a><field_b>price</field_b><value_type>BigNumber</value_type></calculation>",
        "<type>ScriptValueMod</type><jsScripts><jsScript><jsScript_type>0</jsScript_type><jsScript_name>m</jsScript_name><jsScript_script>var amount = 0;\nfor (var i = 0; i &lt; qty; i++) { amount += price; }</jsScript_script></jsScript></jsScripts><fields><field><name>amount</name><rename>amount</rename><type>Number</type><replace>N</replace></field></fields>",
    );
    let p = ekos_lakehouse_kettle::read_package(
        &[ekos_lakehouse_kettle::SourceFile {
            path: "orders.ktr".into(),
            text: ktr,
        }],
        &[],
    );
    let plan = plan(
        &[SourceTable {
            schema: "public".into(),
            oid: 0,
            name: "orders".into(),
            columns: vec![
                col("id", "integer", "integer"),
                col("qty", "integer", "integer"),
                col("price", "numeric(10,2)", "decimal(10,2)"),
            ],
            primary_key: vec!["id".into()],
            row_estimate: 1,
            update_triggers: vec![],
            foreign_keys: vec![],
            comment: None,
        }],
        &PlanSettings::default(),
    );
    let mut s = ekos_lakehouse_pentaho_dbt::Settings {
        package: "etl".into(),
        source_database: Some("shop".into()),
        empty_string_is_null: true,
        ..Default::default()
    };
    let requests = ekos_lakehouse_pentaho_dbt::draft::requests(&p, &plan, &s);
    assert_eq!(requests.len(), 1, "{requests:#?}");
    assert!(requests[0].reason.contains("`for`"));
    let provider = Scripted {
        answers: std::sync::Mutex::new(vec![
            r#"{"columns": {"amount": "\"qty\" * \"price\" * \"discount\""}, "keep_when": null, "cannot": null}"#.into(),
            r#"{"columns": {"amount": "CAST(\"qty\" AS double) * CAST(\"price\" AS double)"}, "keep_when": null, "cannot": null}"#.into(),
        ]),
        prompts: Default::default(),
    };
    let (drafts, outcomes) =
        ekos_lakehouse_ext::pentaho_cmd::draft_all(&requests, &provider, 3).await;
    assert_eq!(outcomes[0].attempts, 2);
    assert!(
        provider.prompts.lock().unwrap()[1].contains("`discount` is not an input field"),
        "the findings go back"
    );
    s.drafts = drafts;
    let t = ekos_lakehouse_pentaho_dbt::translate(&p, &plan, &s);
    let step = t.report.steps.iter().find(|x| x.step == "calc").unwrap();
    assert_eq!(
        step.fidelity,
        ekos_lakehouse_kettle_expr::Fidelity::LlmDrafted,
        "{step:#?}"
    );
    assert!(step.notes.iter().any(|n| n.contains("drafted by scripted")));
    // A drafted step is row-local: it joins the silver model.
    let silver = &t.files["dbt/models/silver/pentaho/etl/pentaho__etl__orders__in.sql"];
    assert!(
        silver.contains("(CAST(\"qty\" AS double) * CAST(\"price\" AS double))"),
        "{silver}"
    );
    assert!(!silver.contains("enabled=false"), "{silver}");
    let m = t
        .report
        .models
        .iter()
        .find(|m| m.name == "pentaho__etl__orders__in")
        .unwrap();
    assert_eq!(m.fidelity, ekos_lakehouse_kettle_expr::Fidelity::LlmDrafted);
}

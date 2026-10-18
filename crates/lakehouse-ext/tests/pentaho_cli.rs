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

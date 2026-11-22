//! EKOS RFC 0268 phase 1's exit on real projects: each Talend fixture translated to a dbt project.
//!
//! The lakehouse plan is **synthetic** (built in the test): one source table per PostgreSQL table a
//! job reads and no job of the project writes, with the columns the job's schema gives it. No model
//! may be disabled by a gate (Trino parse, anti-invention, Java left in SQL): a model is disabled
//! only by a gap in its nodes or a source the plan does not hold.
//!
//! Needs `EKOS_TALEND_FIXTURE_B` … `_E` (EKOS `scripts/talend-fixtures.sh`); each is skipped,
//! saying so, when unset. Third-party projects, never vendored.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
use ekos_lakehouse_bronze::{SourceColumn, SourceTable, TypeKind};
use ekos_lakehouse_etl_expr::KType;
use ekos_lakehouse_etl_rel::{NoCatalog, Op, SourceQuery};
use ekos_lakehouse_talend_dbt::{Layer, Settings, translate};
use ekos_lakehouse_talend_rel::lower_job;
use ekos_talend::{PROJECT_MARKER, Project, ProjectFile, Role, read_project, role};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !ekos_talend::SKIPPED_DIRS.contains(&name.as_str()) {
                walk(&p, out);
            }
        } else {
            out.push(p);
        }
    }
}

fn projects(root: &Path) -> Vec<Project> {
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut out = Vec::new();
    for marker in files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == PROJECT_MARKER))
    {
        let dir = marker.parent().unwrap();
        let mut inside = Vec::new();
        walk(dir, &mut inside);
        let pf: Vec<ProjectFile> = inside
            .iter()
            .filter_map(|p| {
                let rel = p
                    .strip_prefix(dir)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                (role(&rel) != Role::Other).then_some(())?;
                Some(ProjectFile {
                    text: std::fs::read_to_string(p).ok()?,
                    path: rel,
                })
            })
            .collect();
        let rel = dir
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        out.push(read_project(&rel, &pf));
    }
    out
}

fn pg_type(t: KType) -> (&'static str, &'static str) {
    match t {
        KType::Integer => ("bigint", "bigint"),
        KType::Number => ("double precision", "double"),
        KType::BigNumber => ("numeric(38,10)", "decimal(38,10)"),
        KType::Timestamp => ("timestamp", "timestamp(6)"),
        KType::Date => ("date", "date"),
        KType::Boolean => ("boolean", "boolean"),
        _ => ("text", "varchar"),
    }
}

/// The synthetic plan: every PostgreSQL table a job reads that no job writes.
fn synthetic_plan(p: &Project) -> ekos_lakehouse_bronze::plan::Plan {
    let mut written = std::collections::BTreeSet::new();
    let mut read: BTreeMap<(String, String), Vec<(String, KType)>> = BTreeMap::new();
    for job in p.jobs.iter().filter(|j| j.latest) {
        let l = lower_job(&p.expand(job).job, &p.routines, &NoCatalog);
        for n in &l.ir.nodes {
            match &n.op {
                Op::Sink { sink } => {
                    written.insert((
                        sink.target
                            .schema
                            .clone()
                            .unwrap_or_else(|| "public".into())
                            .to_lowercase(),
                        sink.target.table.to_lowercase(),
                    ));
                }
                Op::Source {
                    query: SourceQuery::Sql { tables, .. },
                } => {
                    for t in tables.iter().filter(|t| {
                        t.connection
                            .as_ref()
                            .is_some_and(|c| c.db_type.eq_ignore_ascii_case("postgresql"))
                    }) {
                        let k = (
                            t.schema
                                .clone()
                                .unwrap_or_else(|| "public".into())
                                .to_lowercase(),
                            t.table.to_lowercase(),
                        );
                        let cols = read.entry(k).or_default();
                        for f in &n.schema {
                            if !cols.iter().any(|(c, _)| c.eq_ignore_ascii_case(&f.name)) {
                                cols.push((f.name.to_lowercase(), f.ty));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let tables: Vec<SourceTable> = read
        .into_iter()
        .filter(|(k, _)| !written.contains(k))
        .map(|((schema, name), cols)| SourceTable {
            schema,
            oid: 0,
            name,
            primary_key: cols
                .first()
                .map(|(c, _)| vec![c.clone()])
                .unwrap_or_default(),
            columns: cols
                .iter()
                .map(|(c, t)| {
                    let (pg, trino) = pg_type(*t);
                    SourceColumn {
                        name: c.clone(),
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
                })
                .collect(),
            row_estimate: 10,
            update_triggers: vec![],
            foreign_keys: vec![],
            comment: None,
        })
        .collect();
    plan(&tables, &PlanSettings::default())
}

fn fixture(var: &str) -> Option<PathBuf> {
    let p = std::env::var_os(var).map(PathBuf::from);
    if p.is_none() {
        eprintln!("skipped: {var} is not set (EKOS scripts/talend-fixtures.sh)");
    }
    p
}

const GATES: [&str; 4] = [
    "does not parse as Trino",
    "anti-invention",
    "Java text left",
    "(scope)",
];

/// On a thread with the stack of a process's main thread (8 MiB), as `lakehouse talend translate`
/// runs: a test thread's 2 MiB is too small for the SQL parser on fixture B's long concatenations.
fn check(name: &str, root: &Path) -> Vec<ekos_lakehouse_talend_dbt::Translation> {
    let (name, root) = (name.to_string(), root.to_path_buf());
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(move || check_on_this_thread(&name, &root))
        .unwrap()
        .join()
        .unwrap()
}

fn check_on_this_thread(name: &str, root: &Path) -> Vec<ekos_lakehouse_talend_dbt::Translation> {
    let mut out = Vec::new();
    for p in projects(root) {
        if p.jobs.is_empty() {
            continue;
        }
        let plan = synthetic_plan(&p);
        let t = translate(
            &p,
            &plan,
            &Settings {
                project: p.root.rsplit('/').next().unwrap_or(&p.root).to_string(),
                source_database: None,
                // Stated for the test: fixture B is written in French (its day names, its
                // holidays) and its database is in France.
                locale: ekos_lakehouse_talend_rel::Locale::parse("fr_FR"),
                ..Settings::default()
            },
        );
        let r = &t.report;
        let enabled = r.models.iter().filter(|m| m.enabled).count();
        eprintln!(
            "{name} {}: {} models, {enabled} enabled; questions {}; statements {}; watermarks {}",
            r.project,
            r.models.len(),
            r.questions.len(),
            r.statements.len(),
            r.watermarks.len()
        );
        for ((layer, rule), (n, e)) in r.counts() {
            eprintln!("  {layer:?} {rule:?}: {e}/{n}");
        }
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for q in &r.questions {
            *kinds.entry(q.kind.as_str()).or_default() += 1;
        }
        eprintln!("  questions by kind: {kinds:?}");
        let mut why: BTreeMap<String, usize> = BTreeMap::new();
        for m in r.models.iter().filter(|m| !m.enabled) {
            for g in &m.gaps {
                let short: String = g
                    .split_once(": ")
                    .map_or(g.as_str(), |(_, r)| r)
                    .chars()
                    .take(80)
                    .collect();
                *why.entry(short).or_default() += 1;
            }
        }
        let mut why: Vec<_> = why.into_iter().collect();
        why.sort_by_key(|w| std::cmp::Reverse(w.1));
        for (w, n) in why.iter().take(10) {
            eprintln!("  disabled {n} × {w}");
        }
        for m in &r.models {
            for g in &m.gaps {
                assert!(
                    !GATES.iter().any(|x| g.contains(x)),
                    "{}: a gate disabled {}: {g}",
                    r.project,
                    m.name
                );
            }
            assert!(t.files.contains_key(&m.path));
        }
        out.push(t);
    }
    out
}

#[test]
fn fixture_b_translates_with_no_model_disabled_by_a_gate() {
    let Some(root) = fixture("EKOS_TALEND_FIXTURE_B") else {
        return;
    };
    let all = check("B", &root);
    let dwh = all
        .iter()
        .find(|t| t.report.project == "ALIMENTATION_DATA_WAREHOUSE")
        .expect("the DWH project");
    let r = &dwh.report;
    // Every gold model of a job whose sources the plan holds and whose nodes have no gap is
    // enabled: some are, and each has a file with a header and a config.
    let enabled: Vec<_> = r
        .models
        .iter()
        .filter(|m| m.enabled && m.layer == Layer::Gold)
        .collect();
    assert!(!enabled.is_empty());
    for m in &enabled {
        let sql = &dwh.files[&m.path];
        assert!(sql.starts_with(ekos_lakehouse_talend_dbt::HEADER), "{sql}");
        assert!(sql.contains("{{ config(materialized="), "{sql}");
    }
    assert!(
        dwh.files
            .contains_key("proposals/talend-alimentation_data_warehouse.md")
    );
}

#[test]
fn fixtures_c_d_e_translate_with_no_model_disabled_by_a_gate() {
    for (name, var) in [
        ("C", "EKOS_TALEND_FIXTURE_C"),
        ("D", "EKOS_TALEND_FIXTURE_D"),
        ("E", "EKOS_TALEND_FIXTURE_E"),
    ] {
        let Some(root) = fixture(var) else {
            continue;
        };
        let all = check(name, &root);
        if name == "E" {
            let statements: usize = all.iter().map(|t| t.report.statements.len()).sum();
            assert_eq!(statements, 42);
            // One question per variable; one source reads two (a year and a month).
            let watermarked: std::collections::BTreeSet<(String, String)> = all
                .iter()
                .flat_map(|t| &t.report.questions)
                .filter(|q| q.kind == "watermark")
                .map(|q| (q.job.clone(), q.component.clone()))
                .collect();
            assert_eq!(watermarked.len(), 12);
        }
    }
}

/// EKOS RFC 0268 phase 2's exit on fixture B: its patterns, its refusals, and `--faithful`.
#[test]
fn fixture_b_patterns_are_recognised_refused_by_name_or_turned_off() {
    let Some(root) = fixture("EKOS_TALEND_FIXTURE_B") else {
        return;
    };
    let run = move |faithful: bool| {
        let root = root.clone();
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(move || {
                let p = projects(&root)
                    .into_iter()
                    .find(|p| p.root.ends_with("ALIMENTATION_DATA_WAREHOUSE"))
                    .expect("the DWH project");
                let plan = synthetic_plan(&p);
                translate(
                    &p,
                    &plan,
                    &Settings {
                        project: "dwh".into(),
                        locale: ekos_lakehouse_talend_rel::Locale::parse("fr_FR"),
                        faithful: if faithful {
                            ["*".to_string()].into()
                        } else {
                            Default::default()
                        },
                        ..Settings::default()
                    },
                )
            })
            .unwrap()
            .join()
            .unwrap()
    };
    let t = run(false);
    let r = &t.report;
    let by = |name: &str| {
        r.models
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    use ekos_lakehouse_talend_dbt::Rule;
    // Snapshots: the ODS tables whose three outputs are an SCD2.
    for table in [
        "jodstypeclient__ods_type_client",
        "jodscategorie__ods_categorie",
    ] {
        let gold = by(&format!("talend__dwh__{table}"));
        let snap = by(&format!("talend__dwh__{table}__history"));
        assert_eq!(
            (gold.rule, gold.enabled, snap.enabled),
            (Rule::Snapshot, true, true),
            "{:?}",
            gold.gaps
        );
        let sql = &t.files[&snap.path];
        assert!(
            sql.contains("{% snapshot ") && sql.contains("strategy='check'"),
            "{sql}"
        );
        assert!(t.files[&gold.path].contains("CASE WHEN dbt_valid_to IS NULL THEN 1 ELSE 0 END"));
    }
    let snapshots = r
        .patterns
        .iter()
        .filter(|p| p.rule == Some(Rule::Snapshot))
        .count();
    assert_eq!(snapshots, 5, "{:?}", r.patterns);
    // Refused by name: an all-NULL "close", and a new version of every matched row.
    let refused: Vec<&String> = r
        .patterns
        .iter()
        .filter(|p| p.rule.is_none())
        .flat_map(|p| &p.notes)
        .collect();
    assert!(
        refused
            .iter()
            .any(|n| n.contains("ODS_CLIENT") && n.contains("all NULL but one")),
        "{refused:?}"
    );
    assert!(
        refused
            .iter()
            .any(|n| n.contains("ODS_SOUS_CATEGORIE") && n.contains("changed or not")),
        "{refused:?}"
    );
    // One rejects model for the eighteen writers.
    let rej = by("talend__dwh__rej_ods_rejet");
    assert_eq!(rej.rule, Rule::Rejects);
    let p = r
        .patterns
        .iter()
        .find(|p| p.rule == Some(Rule::Rejects))
        .unwrap();
    assert_eq!(p.components.len(), 18, "{:?}", p.components);
    // The write-back flag has no model, and the calendar is translated.
    assert!(
        r.patterns
            .iter()
            .any(|p| p.rule == Some(Rule::WriteBack) && p.table == "ODS_VENTE")
    );
    assert!(
        !r.models
            .iter()
            .any(|m| m.name == "talend__dwh__jdwhvente__tdboutput_2")
    );
    assert!(by("talend__dwh__jgeneratecalendar__tdboutput_1").enabled);
    // Each listed input is its own source, named by its tFileList's mask (RFC 0268 phase 3).
    let masks: Vec<&str> = r.files.iter().map(|(_, f)| f.as_str()).collect();
    for m in [
        "ICOM_*_TYPE_CLIENT.csv",
        "ICOM_*_CUSTOMER.csv",
        "ICOM_*_VENTE.csv",
    ] {
        assert!(masks.contains(&m), "{masks:?}");
    }
    // --faithful: phase 1's shape, one model per output.
    let f = run(true);
    assert!(
        f.report
            .patterns
            .iter()
            .all(|p| p.rule == Some(Rule::Rejects))
            || f.report.patterns.is_empty(),
        "{:?}",
        f.report.patterns
    );
    assert!(
        !f.report
            .models
            .iter()
            .any(|m| matches!(m.rule, Rule::Snapshot | Rule::Rejects))
    );
    assert!(
        f.report
            .models
            .iter()
            .any(|m| m.name == "talend__dwh__jodstypeclient__tdboutput_3")
    );
}

/// Fixture E: its delete-then-insert jobs on a key are one merge each.
#[test]
fn fixture_e_delete_then_insert_is_a_keyed_replace() {
    let Some(root) = fixture("EKOS_TALEND_FIXTURE_E") else {
        return;
    };
    let all = check("E", &root);
    let keyed = all
        .iter()
        .flat_map(|t| &t.report.patterns)
        .filter(|p| p.rule == Some(ekos_lakehouse_talend_dbt::Rule::KeyedReplace))
        .count();
    // Nine of them join the target inside a chain of joins; FACT_MOV_COST deletes a key it computes
    // and stays refused.
    assert_eq!(keyed, 19);
}

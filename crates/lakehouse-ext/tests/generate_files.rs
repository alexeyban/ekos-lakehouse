//! `generate`'s file rules (RFC 0190 "drift from generated"): an edited file survives
//! regeneration, a table that leaves the plan takes its files with it unless they were edited,
//! and a file that was never generated here is refused.

use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
use ekos_lakehouse_bronze::{SourceColumn, SourceTable, TypeKind};
use ekos_lakehouse_ext::plan_cmd::{edited_files, write};
use ekos_lakehouse_silver::SilverSettings;

fn table(name: &str) -> SourceTable {
    SourceTable {
        schema: "public".into(),
        oid: 0,
        name: name.into(),
        columns: vec![SourceColumn {
            name: "id".into(),
            pg_type: "integer".into(),
            kind: TypeKind::Base,
            not_null: true,
            trino_type: Some("integer".into()),
            numeric_used: None,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
            attnum: 0,
            has_default: false,
        }],
        primary_key: vec!["id".into()],
        row_estimate: 1,
        update_triggers: vec![],
        foreign_keys: vec![],
        comment: None,
    }
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ekos-lh-gen-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn edits_survive_and_dropped_tables_take_their_files() {
    let out = tmp("rules");
    let s = PlanSettings::default();
    let both = plan(&[table("a"), table("b")], &s);
    let w = write(&both, &SilverSettings::default(), &out).unwrap();
    assert!(w.kept.is_empty() && w.removed.is_empty());

    // A person edits table a's model and b's load.
    let a_model = out.join("dbt/models/silver/public/public__a.sql");
    let b_load = out.join("bronze/public/b/load.sql");
    std::fs::write(
        &a_model,
        std::fs::read_to_string(&a_model).unwrap() + "-- note\n",
    )
    .unwrap();
    std::fs::write(
        &b_load,
        std::fs::read_to_string(&b_load).unwrap() + "-- note\n",
    )
    .unwrap();
    assert_eq!(edited_files(&out).len(), 2);

    // Regenerate without b: a's edit is kept; b's untouched files go, its edited one stays.
    let only_a = plan(&[table("a")], &s);
    let w = write(&only_a, &SilverSettings::default(), &out).unwrap();
    assert!(
        w.kept
            .contains(&"dbt/models/silver/public/public__a.sql".to_string()),
        "{:?}",
        w.kept
    );
    assert!(
        w.kept.contains(&"bronze/public/b/load.sql".to_string()),
        "{:?}",
        w.kept
    );
    assert!(
        w.removed
            .contains(&"dbt/models/silver/public/public__b.sql".to_string()),
        "{:?}",
        w.removed
    );
    assert!(
        std::fs::read_to_string(&a_model)
            .unwrap()
            .ends_with("-- note\n")
    );
    assert!(!out.join("dbt/models/silver/public/public__b.sql").exists());
    assert!(b_load.exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_file_never_generated_here_is_refused() {
    let out = tmp("foreign");
    std::fs::create_dir_all(out.join("bronze/public/a")).unwrap();
    std::fs::write(out.join("bronze/public/a/load.sql"), "select 1\n").unwrap();
    let err = write(
        &plan(&[table("a")], &PlanSettings::default()),
        &SilverSettings::default(),
        &out,
    )
    .err()
    .unwrap()
    .to_string();
    assert!(err.contains("bronze/public/a/load.sql"), "{err}");
    assert_eq!(
        std::fs::read_to_string(out.join("bronze/public/a/load.sql")).unwrap(),
        "select 1\n"
    );
    let _ = std::fs::remove_dir_all(&out);
}

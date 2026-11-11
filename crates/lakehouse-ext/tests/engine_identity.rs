//! RFC 0255 phase 1 exit: turning the closed `SourceEngine` enum into an open seam changes no
//! output. Two lock files written before the seam — Pagila (PostgreSQL, no engine field) and a
//! slice of the published iDempiere demo (Oracle) — must read back and re-serialize byte for byte,
//! and every generated pipeline file and every reconciliation query must hash as it did before.
//!
//! The digests in `fixtures/rfc0255/*.sha256` were recorded on the commit before the seam
//! (`RFC0255_RECORD=1 cargo test -p ekos-lakehouse-ext --test engine_identity`). A full lock can
//! be checked the same way, outside the repository: `RFC0255_EXTRA_LOCK=<path>` prints its
//! combined digest, for comparing two builds by hand.

use ekos_lakehouse_bronze::plan::Plan;
use ekos_migrate_validate::dialect::{
    Dialect, aggregate_query, bucket_checksum_query, count_query,
};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rfc0255")
        .join(name)
}

fn sha(text: &str) -> String {
    ekos_lakehouse_ext::plan_cmd::sha256(text)
}

/// Every generated file and every reconciliation query of `plan`, as `sha256  path` lines.
fn digests(plan: &Plan) -> String {
    let mut lines = Vec::new();
    for (path, content) in ekos_lakehouse_bronze::render::files(plan) {
        lines.push(format!("{}  {path}", sha(&content)));
    }
    let (silver, _) =
        ekos_lakehouse_silver::files(plan, &ekos_lakehouse_silver::SilverSettings::default());
    for (path, content) in silver {
        lines.push(format!("{}  silver:{path}", sha(&content)));
    }
    let src = ekos_lakehouse_ext::engines::Engines::builtin()
        .get(&plan.settings.source_engine)
        .unwrap()
        .dialect();
    for t in &plan.tables {
        let (u, not_compared) = ekos_lakehouse_ext::reconcile::unit_plan(t, &plan.settings, src);
        let sql = [
            count_query(src, &u.source_table),
            count_query(Dialect::Trino, &u.target_table),
            aggregate_query(src, &u.source_table, &u.source_columns),
            aggregate_query(Dialect::Trino, &u.target_table, &u.target_columns),
            bucket_checksum_query(src, &u.source_table, &u.source_pk, &u.source_columns, 64),
            bucket_checksum_query(
                Dialect::Trino,
                &u.target_table,
                &u.target_pk,
                &u.target_columns,
                64,
            ),
            format!(
                "digest_columns={} not_compared={not_compared:?}",
                u.digest_columns
            ),
        ]
        .join("\n");
        lines.push(format!("{}  reconcile:{}.{}", sha(&sql), t.schema, t.table));
    }
    lines.sort();
    lines.join("\n") + "\n"
}

fn check(lock: &str, golden: &str) {
    let text = std::fs::read_to_string(fixture(lock)).unwrap();
    let plan: Plan = serde_json::from_str(&text).unwrap();
    // The lock as `generate` writes it is the lock read back.
    let again = serde_json::to_string_pretty(&plan).unwrap() + "\n";
    let got = digests(&plan);
    if std::env::var("RFC0255_RECORD").is_ok_and(|v| v == "1") {
        // Recording normalizes the lock to what the pre-seam code writes.
        std::fs::write(fixture(lock), &again).unwrap();
        std::fs::write(fixture(golden), &got).unwrap();
        return;
    }
    assert!(again == text, "{lock} does not re-serialize byte for byte");
    let want = std::fs::read_to_string(fixture(golden)).unwrap();
    assert!(
        got == want,
        "{lock}: generated output changed:\n{}",
        diff(&want, &got)
    );
}

fn diff(want: &str, got: &str) -> String {
    let w: std::collections::BTreeSet<&str> = want.lines().collect();
    let g: std::collections::BTreeSet<&str> = got.lines().collect();
    let mut out: Vec<String> = w.difference(&g).map(|l| format!("- {l}")).collect();
    out.extend(g.difference(&w).map(|l| format!("+ {l}")));
    out.join("\n")
}

#[test]
fn a_postgres_lock_from_before_the_seam_is_unchanged() {
    let text = std::fs::read_to_string(fixture("pagila.lock.json")).unwrap();
    assert!(
        !text.contains("source-engine"),
        "a PostgreSQL lock names no engine"
    );
    check("pagila.lock.json", "pagila.sha256");
}

#[test]
fn an_oracle_lock_from_before_the_seam_is_unchanged() {
    let text = std::fs::read_to_string(fixture("idempiere.lock.json")).unwrap();
    assert!(text.contains("\"source-engine\": \"oracle\""));
    check("idempiere.lock.json", "idempiere.sha256");
}

/// Not a check: prints the combined digest of a lock outside the repository (the full iDempiere
/// demo lock is 12 MB), so two builds can be compared by hand.
#[test]
fn print_the_digest_of_an_extra_lock() {
    let Ok(path) = std::env::var("RFC0255_EXTRA_LOCK") else {
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap();
    let plan: Plan = serde_json::from_str(&text).unwrap();
    let again = serde_json::to_string_pretty(&plan).unwrap() + "\n";
    let d = digests(&plan);
    eprintln!(
        "RFC0255 {path}: round trip {}, {} lines, digest {}",
        if again == text {
            "identical"
        } else {
            "DIFFERENT"
        },
        d.lines().count(),
        sha(&d)
    );
}

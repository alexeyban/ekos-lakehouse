//! RFC 0225 §2.4: deleted rows without a deletion marker, found by a bucketed key diff.
//!
//! Every key is hashed into buckets on both sides — the source read directly in its own dialect,
//! bronze's current state through Trino — and only the buckets whose counts or key-hash sums
//! differ are read key by key. A key bronze holds and the source lacks was deleted at the source.
//! Before anything is applied, the mass-delete guard asks whether that many vanishing keys is
//! believable: a lost permission, a truncated table and a wrong database look like mass deletes.

use crate::reconcile::{key_expr, rule};
use ekos_lakehouse_bronze::plan::{PlanSettings, TablePlan};
use ekos_lakehouse_bronze::render;
use ekos_lakehouse_bronze::typemap::ident;
use ekos_migrate_validate::dialect::{ColumnRule, Dialect, bucket_checksum_query, bucket_expr};
use ekos_migrate_validate::{EngineReader, ReadError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Buckets per table: unchanged buckets cost one aggregate row each side.
pub const BUCKETS: u32 = 256;

/// Keys per `IN (…)` list: Oracle takes at most 1,000.
const IN_LIST: usize = 500;

/// What one check found, before the guard decides whether to apply it.
#[derive(Debug, Clone, Default)]
pub struct Found {
    pub buckets: u32,
    pub differing: Vec<u32>,
    pub source_keys_read: usize,
    pub bronze_keys_read: usize,
    /// Keys in bronze's current state.
    pub bronze_keys: u64,
    /// Canonical keys bronze holds and the source lacks. Held in memory for the tombstones only:
    /// never printed, never recorded.
    pub deleted: Vec<String>,
    /// Keys the source has that bronze has not loaded yet (the next load brings them).
    pub unloaded: usize,
}

/// The fact a check leaves (`LakehouseDeleteCheck`): counts and times only, never a key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Check {
    pub table: String,
    pub run: String,
    pub method: String,
    pub kind: String,
    pub buckets: u32,
    pub differing_buckets: usize,
    pub source_keys_read: usize,
    pub bronze_keys_read: usize,
    pub bronze_keys: u64,
    pub found: usize,
    pub applied: usize,
    pub unloaded: usize,
    /// Why nothing was applied, when the guard stopped it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
    /// The code says this table is never hard-deleted, and a delete was found.
    pub contradicts_code: bool,
    /// The interval the deletes happened in: the previous check, and this one. The true time is
    /// unknown without CDC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    pub detected_at: String,
}

/// The key columns with their compare rules, or why the key cannot be compared.
fn key_rules(t: &TablePlan) -> anyhow::Result<Vec<(String, ColumnRule)>> {
    t.primary_key
        .iter()
        .map(|k| {
            let m = t
                .columns
                .iter()
                .find(|m| &m.column == k)
                .ok_or_else(|| anyhow::anyhow!("key column {k} is not landed"))?;
            let c = m.compare.ok_or_else(|| {
                anyhow::anyhow!("key column {k} ({}) has no canonical form", m.pg_type)
            })?;
            Ok((k.clone(), rule(c)))
        })
        .collect()
}

fn buckets_of(r: &dyn EngineReader, sql: &str) -> Result<BTreeMap<u32, (u64, String)>, ReadError> {
    let mut out = BTreeMap::new();
    for row in r.query(sql)? {
        let bucket: u32 = row[0].parse().map_err(|_| ReadError::Parse {
            value: row[0].clone(),
            as_type: "u32",
            column: 0,
        })?;
        let count: u64 = row[1].parse().map_err(|_| ReadError::Parse {
            value: row[1].clone(),
            as_type: "u64",
            column: 1,
        })?;
        out.insert(bucket, (count, row[2].clone()));
    }
    Ok(out)
}

fn keys_in(
    r: &dyn EngineReader,
    d: Dialect,
    table: &str,
    pk: &str,
    buckets: &[u32],
) -> Result<BTreeSet<String>, ReadError> {
    let mut keys = BTreeSet::new();
    let b = bucket_expr(d, pk, BUCKETS);
    for chunk in buckets.chunks(IN_LIST) {
        let list = chunk
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        for row in r.query(&format!("SELECT {pk} FROM {table} WHERE {b} IN ({list})"))? {
            keys.insert(row.into_iter().next().unwrap_or_default());
        }
    }
    Ok(keys)
}

/// The bucketed key diff for one table.
pub fn find(
    t: &TablePlan,
    s: &PlanSettings,
    source: &dyn EngineReader,
    bronze: &dyn EngineReader,
) -> anyhow::Result<Found> {
    let key = key_rules(t)?;
    let src = source.dialect();
    let (src_pk, tgt_pk) = (key_expr(src, &key), key_expr(Dialect::Trino, &key));
    let src_table = format!("{}.{}", ident(&t.schema), ident(&t.table));
    let tgt_table = format!("({}) AS t", render::current_sql(t, s));
    let s_b = buckets_of(
        source,
        &bucket_checksum_query(
            src,
            &src_table,
            &src_pk,
            std::slice::from_ref(&src_pk),
            BUCKETS,
        ),
    )?;
    let t_b = buckets_of(
        bronze,
        &bucket_checksum_query(
            Dialect::Trino,
            &tgt_table,
            &tgt_pk,
            std::slice::from_ref(&tgt_pk),
            BUCKETS,
        ),
    )?;
    let mut found = Found {
        buckets: BUCKETS,
        bronze_keys: t_b.values().map(|(n, _)| n).sum(),
        ..Default::default()
    };
    let all: BTreeSet<u32> = s_b.keys().chain(t_b.keys()).copied().collect();
    found.differing = all
        .into_iter()
        .filter(|b| s_b.get(b) != t_b.get(b))
        .collect();
    if found.differing.is_empty() {
        return Ok(found);
    }
    let src_keys = keys_in(source, src, &src_table, &src_pk, &found.differing)?;
    let tgt_keys = keys_in(
        bronze,
        Dialect::Trino,
        &tgt_table,
        &tgt_pk,
        &found.differing,
    )?;
    found.source_keys_read = src_keys.len();
    found.bronze_keys_read = tgt_keys.len();
    found.deleted = tgt_keys.difference(&src_keys).cloned().collect();
    found.unloaded = src_keys.difference(&tgt_keys).count();
    Ok(found)
}

/// The mass-delete guard (plan III.4, D9): `Some(reason)` when this many vanishing keys must be
/// confirmed by a person before anything is applied.
pub fn guard(deleted: usize, bronze_keys: u64, percent: u32, rows: u64) -> Option<String> {
    let n = deleted as u64;
    if n > rows {
        return Some(format!(
            "{n} keys vanished, more than the {rows} the guard allows in one check"
        ));
    }
    if bronze_keys > 0 && n * 100 > u64::from(percent) * bronze_keys {
        return Some(format!(
            "{n} of {bronze_keys} keys vanished, more than {percent}%: a lost permission, a \
             truncated table or a wrong database look the same from here"
        ));
    }
    None
}

/// The tombstone statements for `deleted` keys, matched on bronze's canonical key expression.
pub fn tombstone_statements(
    t: &TablePlan,
    s: &PlanSettings,
    op: &str,
    deleted: &[String],
) -> anyhow::Result<Vec<String>> {
    let key = key_rules(t)?;
    let pk = key_expr(Dialect::Trino, &key);
    Ok(deleted
        .chunks(IN_LIST)
        .map(|chunk| {
            let list = chunk
                .iter()
                .map(|k| format!("'{}'", k.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");
            render::tombstones_where(t, s, op, &format!("{pk} IN ({list})"))
        })
        .collect())
}

/// When each table was last checked: `<state>/lakehouse/deletes.json`. Times only.
pub fn read_state(path: &std::path::Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn write_state(path: &std::path::Path, state: &BTreeMap<String, String>) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(state)? + "\n")?;
    Ok(())
}

/// Whether an audit is due: never checked, or checked at least `days` ago.
pub fn due(last: Option<&str>, days: u32, now: chrono::DateTime<chrono::Utc>) -> bool {
    last.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_none_or(|t| now.signed_duration_since(t) >= chrono::Duration::days(i64::from(days)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_trips_on_either_threshold() {
        assert_eq!(guard(4, 100, 5, 10_000), None);
        assert!(guard(6, 100, 5, 10_000).unwrap().contains("6 of 100"));
        assert!(
            guard(10_001, 10_000_000, 5, 10_000)
                .unwrap()
                .contains("more than the 10000")
        );
        // Everything gone from a table bronze holds: the guard, whatever the size.
        assert!(guard(3, 3, 5, 10_000).is_some());
    }

    #[test]
    fn an_audit_is_due_after_its_interval() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-25T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(due(None, 7, now));
        assert!(!due(Some("2026-10-20T12:00:00Z"), 7, now));
        assert!(due(Some("2026-10-18T12:00:00Z"), 7, now));
    }

    fn plan_of() -> (TablePlan, PlanSettings) {
        let lock: serde_json::Value = serde_json::json!({
            "schema": "ADEMPIERE", "table": "C_ORDER", "namespace": "bronze_ADEMPIERE",
            "primary_key": ["C_ORDER_ID"],
            "decision": {"mode": {"mode": "incremental", "watermark": "UPDATED", "keyset_deletes": false},
                         "evidence": [], "notes": []},
            "columns": [
                {"column": "C_ORDER_ID", "pg_type": "NUMBER(10,0)", "trino_type": "decimal(10,0)",
                 "iceberg_type": "bigint", "expr": "CAST(\"C_ORDER_ID\" AS bigint)", "lossiness": "exact",
                 "rule": "number-bigint", "rationale": "", "compare": {"rule": "int"}},
                {"column": "UPDATED", "pg_type": "DATE", "trino_type": "timestamp(0)",
                 "iceberg_type": "timestamp(6)", "expr": "CAST(\"UPDATED\" AS timestamp(6))",
                 "lossiness": "widening", "rule": "oracle-date", "rationale": "",
                 "compare": {"rule": "timestamp_naive"}}
            ]
        });
        (
            serde_json::from_value(lock).unwrap(),
            PlanSettings::default(),
        )
    }

    /// Answers by the first needle the SQL contains, and records what was asked.
    struct Fake {
        dialect: Dialect,
        answers: Vec<(&'static str, Vec<Vec<String>>)>,
        seen: std::cell::RefCell<Vec<String>>,
    }

    impl EngineReader for Fake {
        fn dialect(&self) -> Dialect {
            self.dialect
        }
        fn label(&self) -> &str {
            "fake"
        }
        fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, ReadError> {
            self.seen.borrow_mut().push(sql.to_string());
            Ok(self
                .answers
                .iter()
                .find(|(n, _)| sql.contains(n))
                .map(|(_, r)| r.clone())
                .unwrap_or_default())
        }
    }

    fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
        r.iter()
            .map(|row| row.iter().map(|c| c.to_string()).collect())
            .collect()
    }

    /// Buckets that agree are never read key by key; a key bronze has and the source lacks is
    /// the delete, one the source has and bronze lacks is not yet loaded.
    #[test]
    fn only_differing_buckets_are_read() {
        let (t, s) = plan_of();
        let src = Fake {
            dialect: Dialect::Oracle,
            answers: vec![
                ("AS row_count", rows(&[&["1", "2", "10"], &["7", "1", "5"]])),
                (") IN (", rows(&[&["10"], &["11"], &["12"]])),
            ],
            seen: Default::default(),
        };
        let tgt = Fake {
            dialect: Dialect::Trino,
            answers: vec![
                ("AS row_count", rows(&[&["1", "3", "17"], &["7", "1", "5"]])),
                (" IN (", rows(&[&["10"], &["11"], &["13"], &["14"]])),
            ],
            seen: Default::default(),
        };
        let f = find(&t, &s, &src, &tgt).unwrap();
        assert_eq!(f.differing, [1]);
        assert_eq!(f.bronze_keys, 4);
        assert_eq!(f.deleted, ["13", "14"]);
        assert_eq!(f.unloaded, 1);
        let read = src.seen.borrow();
        assert!(read.iter().any(|q| q.ends_with("IN (1)")), "{read:?}");
        assert!(
            !read.iter().any(|q| q.contains("IN (7)")),
            "an agreeing bucket was read: {read:?}"
        );
    }

    #[test]
    fn tombstones_name_keys_only_through_the_canonical_key() {
        let (t, s) = plan_of();
        let stmts = tombstone_statements(&t, &s, "delete", &["13".into(), "o'k".into()]).unwrap();
        assert_eq!(stmts.len(), 1);
        let q = &stmts[0];
        assert!(q.contains("IN ('13', 'o''k')"), "{q}");
        assert!(
            q.contains("CAST(NULL AS timestamp(6)) AS \"UPDATED\""),
            "{q}"
        );
        assert!(q.contains("CAST('delete' AS varchar)"), "{q}");
    }
}

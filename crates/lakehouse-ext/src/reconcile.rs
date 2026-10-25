//! RFC 0192 §6: every bronze table reconciled against its source on every run, with a planted
//! control proving the reconciliation can fail.
//!
//! The source is read directly from PostgreSQL, bronze through the read-only `TrinoReader`
//! (RFC 0156's independent-oracle rule). An incremental table is compared through its current
//! state: the latest version of each key, tombstones removed.

use ekos_lakehouse_bronze::plan::{PlanSettings, TablePlan};
use ekos_lakehouse_bronze::typemap::ident;
use ekos_lakehouse_bronze::{CompareRule, SourceEngine, render};
use ekos_migrate_validate::dialect::{
    ColumnRule, Dialect, bucket_checksum_query, canon_expr, count_query,
};
use ekos_migrate_validate::tiers::{UnitPlan, run_v1, run_v2, run_v3};
use ekos_migrate_validate::{EngineReader, ReadError};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every tier agreed and the planted control fired.
    Green,
    /// A tier found a difference.
    Diverged,
    /// The tiers agreed but the control did not fire: agreement proves nothing.
    Unverified,
}

#[derive(Debug, Clone, Serialize)]
pub struct TierResult {
    pub tier: &'static str,
    pub divergences: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reconciliation {
    pub table: String,
    pub tiers: Vec<TierResult>,
    /// Columns without a canonical rule (arrays, floats), named rather than silently skipped.
    pub not_compared: Vec<String>,
    /// `Some(true)`: the planted control was detected, as it must be. `None`: not applicable
    /// (no rows to plant in).
    pub control_fired: Option<bool>,
    pub control_detail: String,
    pub verdict: Verdict,
}

fn rule(c: CompareRule) -> ColumnRule {
    match c {
        CompareRule::Text => ColumnRule::Text,
        CompareRule::Bool => ColumnRule::Bool,
        CompareRule::Int => ColumnRule::Int,
        CompareRule::Decimal(s) => ColumnRule::Decimal(s),
        CompareRule::TimestampUtc => ColumnRule::TimestampUtc,
        CompareRule::TimestampNaive => ColumnRule::TimestampNaive,
        CompareRule::Date => ColumnRule::Date,
        CompareRule::Time => ColumnRule::Time,
        CompareRule::Uuid => ColumnRule::Uuid,
        CompareRule::Bytes => ColumnRule::Bytes,
    }
}

/// The key's canonical expression: its columns joined by the unit separator. Without a primary
/// key, every compared column is the key — V3 then compares the rows as a multiset.
fn key_expr(d: Dialect, cols: &[(String, ColumnRule)]) -> String {
    let parts: Vec<String> = cols.iter().map(|(c, r)| canon_expr(d, c, *r)).collect();
    parts.join(" || chr(31) || ")
}

/// The dialect a plan's source is read in (RFC 0216).
pub fn source_dialect(s: &PlanSettings) -> Dialect {
    match s.source_engine {
        SourceEngine::Postgres => Dialect::Postgres,
        SourceEngine::Oracle => Dialect::Oracle,
    }
}

pub fn unit_plan(t: &TablePlan, s: &PlanSettings) -> (UnitPlan, Vec<String>) {
    let src = source_dialect(s);
    let compared: Vec<(String, ColumnRule)> = t
        .columns
        .iter()
        .filter_map(|m| m.compare.map(|c| (m.column.clone(), rule(c))))
        .collect();
    let not_compared = t
        .columns
        .iter()
        .filter(|m| m.compare.is_none())
        .map(|m| format!("{} ({})", m.column, m.pg_type))
        .collect();
    let key: Vec<(String, ColumnRule)> = if t.primary_key.is_empty() {
        compared.clone()
    } else {
        compared
            .iter()
            .filter(|(c, _)| t.primary_key.contains(c))
            .cloned()
            .collect()
    };
    let unit = UnitPlan {
        unit: format!("{}.{}", t.schema, t.table),
        source_table: format!("{}.{}", ident(&t.schema), ident(&t.table)),
        target_table: format!("({}) AS t", render::current_sql(t, s)),
        source_pk: key_expr(src, &key),
        target_pk: key_expr(Dialect::Trino, &key),
        source_columns: compared
            .iter()
            .map(|(c, r)| canon_expr(src, c, *r))
            .collect(),
        target_columns: compared
            .iter()
            .map(|(c, r)| canon_expr(Dialect::Trino, c, *r))
            .collect(),
        buckets: 64,
        // Oracle cannot build a row string past 4000 bytes (RFC 0216 §2.5).
        digest_columns: src == Dialect::Oracle,
    };
    (unit, not_compared)
}

fn describe(d: &[ekos_migrate_validate::divergence::Divergence]) -> Vec<String> {
    d.iter()
        .map(|x| format!("{}: {}", x.locus, x.detail))
        .collect()
}

/// V1–V3, then the control: V3 again with one row's last compared column changed in the target
/// query. If V3 does not flag it, the run is unverified.
pub fn reconcile(
    t: &TablePlan,
    s: &PlanSettings,
    source: &dyn EngineReader,
    target: &dyn EngineReader,
) -> Result<Reconciliation, ReadError> {
    let (unit, not_compared) = unit_plan(t, s);
    let names: Vec<String> = t
        .columns
        .iter()
        .filter(|m| m.compare.is_some())
        .map(|m| m.column.clone())
        .collect();
    // A value a side cannot render canonically (a NULL bucket, an unparseable aggregate) is a
    // difference to report, not a reason to stop: LedgerSMB's `infinity` timestamp, misread by
    // the connector, surfaced first as "cannot parse '' as u32".
    let as_result =
        |tier: &'static str,
         r: Result<Vec<ekos_migrate_validate::divergence::Divergence>, ReadError>| {
            TierResult {
                tier,
                divergences: match r {
                    Ok(o) => describe(&o),
                    Err(ReadError::Parse {
                        value,
                        as_type,
                        column,
                    }) => vec![format!(
                        "{}: a side returned {value:?} where the tier expected {as_type} (column {column}) \
                 — a value it cannot render canonically",
                        unit.unit
                    )],
                    Err(e) => vec![format!("{}: {e}", unit.unit)],
                },
            }
        };
    let tiers = vec![
        as_result("V1", run_v1(&unit, source, target).map(|o| o.divergences)),
        as_result(
            "V2",
            run_v2(&unit, source, target, &names).map(|o| o.divergences),
        ),
        as_result("V3", run_v3(&unit, source, target).map(|o| o.divergences)),
    ];
    let diverged = tiers.iter().any(|t| !t.divergences.is_empty());

    // The control: plant a difference in one existing row, in the query only. Pointless once a
    // tier already diverged: that run has proved the comparison can fail.
    if diverged {
        return Ok(Reconciliation {
            table: unit.unit,
            tiers,
            not_compared,
            control_fired: None,
            control_detail: "not run: the tiers already found a difference".into(),
            verdict: Verdict::Diverged,
        });
    }
    let first_key = target.query(&format!(
        "SELECT min({}) FROM {}",
        unit.target_pk, unit.target_table
    ))?;
    let key = first_key
        .first()
        .and_then(|r| r.first())
        .filter(|k| !k.is_empty())
        .cloned();
    let (control_fired, control_detail) = match (key, unit.target_columns.last()) {
        (Some(key), Some(last)) => {
            let lit = format!("'{}'", key.replace('\'', "''"));
            let n = unit.target_columns.len();
            let mut target_columns = unit.target_columns.clone();
            target_columns[n - 1] = format!(
                "(CASE WHEN {} = {lit} THEN {last} || 'x' ELSE {last} END)",
                unit.target_pk
            );
            let planted = UnitPlan {
                unit: unit.unit.clone(),
                source_table: unit.source_table.clone(),
                target_table: unit.target_table.clone(),
                source_pk: unit.source_pk.clone(),
                target_pk: unit.target_pk.clone(),
                source_columns: unit.source_columns.clone(),
                target_columns,
                buckets: unit.buckets,
                digest_columns: unit.digest_columns,
            };
            match run_v3(&planted, source, target) {
                Ok(o) => (
                    Some(!o.divergences.is_empty()),
                    format!("one row's {} changed in the query", names[n - 1]),
                ),
                Err(e) => (Some(false), format!("the control could not be read: {e}")),
            }
        }
        _ => (None, "no rows to plant a difference in".into()),
    };
    let verdict = if control_fired == Some(false) {
        Verdict::Unverified
    } else {
        Verdict::Green
    };
    Ok(Reconciliation {
        table: unit.unit,
        tiers,
        not_compared,
        control_fired,
        control_detail,
        verdict,
    })
}

/// The queries a reconciliation runs, for the fact's evidence: counts and bucket checksums on
/// both sides. They read aggregates only.
pub fn evidence_queries(t: &TablePlan, s: &PlanSettings) -> Vec<String> {
    let (u, _) = unit_plan(t, s);
    let src = source_dialect(s);
    vec![
        count_query(src, &u.source_table),
        count_query(Dialect::Trino, &u.target_table),
        bucket_checksum_query(
            src,
            &u.source_table,
            &u.source_pk,
            &u.source_row(src),
            u.buckets,
        ),
        bucket_checksum_query(
            Dialect::Trino,
            &u.target_table,
            &u.target_pk,
            &u.target_row(Dialect::Trino),
            u.buckets,
        ),
    ]
}

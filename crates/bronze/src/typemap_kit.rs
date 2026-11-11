//! RFC 0255 §3.4 — the type-map kit: what an engine's type map must hold before it plans bronze.
//!
//! An engine's author lists sample columns covering its types (with the connector type Trino
//! reports, and a measurement where the map needs one), and [`check`] holds the map to the rules
//! every built-in engine follows:
//!
//! - **every column is mapped or refused, never dropped**: the mapping names the column and its
//!   source type as given, and a mapped column has an Iceberg type and a load expression;
//! - **`NarrowingSafe` only with a measurement**: a narrower landing is safe only for the values
//!   measured (`numeric_used`), never assumed;
//! - **a refused column refuses its table**, naming the column, and the plan keeps every column
//!   of a table it lands;
//! - **a compare rule only on a landed column**: a refused column is not reconciled.
//!
//! Pure, like the rest of the crate: no connection.

use crate::engine::EngineSpec;
use crate::model::{SourceColumn, SourceTable, TypeKind};
use crate::plan::{PlanSettings, plan_for};
use crate::typemap::Lossiness;
use std::collections::BTreeMap;

/// One sample column: the source type as the engine spells it, the connector's type, and an
/// optional measurement (precision, scale actually used).
#[derive(Debug, Clone)]
pub struct Sample {
    pub source_type: String,
    pub trino_type: Option<String>,
    pub numeric_used: Option<(u32, u32)>,
    pub kind: TypeKind,
}

impl Sample {
    pub fn new(source_type: &str, trino_type: Option<&str>) -> Self {
        Self {
            source_type: source_type.into(),
            trino_type: trino_type.map(str::to_string),
            numeric_used: None,
            kind: TypeKind::Base,
        }
    }

    pub fn measured(mut self, precision: u32, scale: u32) -> Self {
        self.numeric_used = Some((precision, scale));
        self
    }

    fn column(&self, i: usize) -> SourceColumn {
        SourceColumn {
            name: format!("C{i}"),
            pg_type: self.source_type.clone(),
            kind: self.kind.clone(),
            not_null: false,
            trino_type: self.trino_type.clone(),
            numeric_used: self.numeric_used,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
            attnum: i as i32 + 1,
            has_default: false,
        }
    }
}

/// What the kit found: one line per broken rule, naming the sample.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KitReport {
    pub checked: usize,
    pub refused: usize,
    pub failures: Vec<String>,
}

impl KitReport {
    pub fn passed(&self) -> bool {
        self.failures.is_empty() && self.checked > 0
    }
}

/// Hold `spec`'s type map to the kit's rules over `samples`.
pub fn check(spec: &dyn EngineSpec, samples: &[Sample]) -> KitReport {
    let mut r = KitReport::default();
    let settings = PlanSettings {
        source_engine: spec.engine(),
        ..Default::default()
    };
    for (i, s) in samples.iter().enumerate() {
        r.checked += 1;
        let col = s.column(i);
        let what = format!("{} ({:?})", s.source_type, s.trino_type);
        let m = spec.map_column(&col);
        if m.column != col.name || m.pg_type != col.pg_type {
            r.failures.push(format!(
                "{what}: mapped as {} ({}), not as the column given",
                m.column, m.pg_type
            ));
        }
        if m.lossiness == Lossiness::Refused {
            r.refused += 1;
            if m.rationale.trim().is_empty() {
                r.failures
                    .push(format!("{what}: refused without saying why"));
            }
            if m.compare.is_some() {
                r.failures
                    .push(format!("{what}: refused, yet given a compare rule"));
            }
            // A refused column refuses its table, naming the column.
            let p = plan_for(
                spec,
                &[table(vec![col.clone()])],
                &settings,
                &BTreeMap::new(),
            );
            let named = p
                .refused
                .first()
                .is_some_and(|t| t.reasons.iter().any(|x| x.starts_with(&col.name)));
            if !p.tables.is_empty() || !named {
                r.failures.push(format!(
                    "{what}: refused, but its table was planned or the refusal does not name it"
                ));
            }
            continue;
        }
        if m.iceberg_type.trim().is_empty() || m.expr.trim().is_empty() {
            r.failures.push(format!(
                "{what}: dropped — neither refused nor given an Iceberg type and a load expression"
            ));
        }
        if m.lossiness == Lossiness::NarrowingSafe && col.numeric_used.is_none() {
            r.failures
                .push(format!("{what}: narrowing_safe without a measurement"));
        }
        // The plan keeps the column.
        let p = plan_for(
            spec,
            &[table(vec![col.clone()])],
            &settings,
            &BTreeMap::new(),
        );
        if p.tables.first().map(|t| t.columns.len()) != Some(1) {
            r.failures.push(format!(
                "{what}: mapped, but the plan did not keep the column"
            ));
        }
    }
    r
}

fn table(columns: Vec<SourceColumn>) -> SourceTable {
    SourceTable {
        schema: "KIT".into(),
        name: "T".into(),
        oid: 1,
        columns,
        primary_key: vec![],
        row_estimate: 1,
        update_triggers: vec![],
        foreign_keys: vec![],
        comment: None,
    }
}

/// Samples covering the PostgreSQL type map (the Pagila measurement, RFC 0192).
pub fn postgres_samples() -> Vec<Sample> {
    vec![
        Sample::new("integer", Some("integer")),
        Sample::new("bigint", Some("bigint")),
        Sample::new("smallint", Some("smallint")),
        Sample::new("numeric(5,2)", Some("decimal(5,2)")),
        Sample::new("numeric", Some("decimal(38,0)")),
        Sample::new("numeric", Some("decimal(38,0)")).measured(10, 2),
        Sample::new("text", Some("varchar")),
        Sample::new("character varying(45)", Some("varchar(45)")),
        Sample::new("character(20)", Some("char(20)")),
        Sample::new("boolean", Some("boolean")),
        Sample::new("date", Some("date")),
        Sample::new("timestamp without time zone", Some("timestamp(6)")),
        Sample::new(
            "timestamp with time zone",
            Some("timestamp(6) with time zone"),
        ),
        Sample::new("uuid", Some("uuid")),
        Sample::new("bytea", Some("varbinary")),
        Sample::new("tsvector", Some("varchar")),
        Sample::new("interval", Some("varchar")),
        Sample::new("double precision", Some("double")),
        Sample::new("jsonb", Some("json")),
        Sample::new("text", None),
    ]
}

/// Samples covering the Oracle type map (RFC 0216 §2.4).
pub fn oracle_samples() -> Vec<Sample> {
    vec![
        Sample::new("NUMBER(10,0)", Some("decimal(10,0)")),
        Sample::new("NUMBER(9,0)", Some("decimal(9,0)")),
        Sample::new("NUMBER(12,2)", Some("decimal(12,2)")),
        Sample::new("NUMBER", Some("decimal(38,0)")),
        Sample::new("NUMBER", Some("decimal(38,0)")).measured(12, 2),
        Sample::new("INTEGER", Some("decimal(38,0)")),
        Sample::new("VARCHAR2(60 CHAR)", Some("varchar(60)")),
        Sample::new("NVARCHAR2(60)", Some("varchar(60)")),
        Sample::new("CHAR(1)", Some("char(1)")),
        Sample::new("CLOB", Some("varchar")),
        Sample::new("DATE", Some("timestamp(0)")),
        Sample::new("TIMESTAMP(6)", Some("timestamp(6)")),
        Sample::new(
            "TIMESTAMP(6) WITH TIME ZONE",
            Some("timestamp(6) with time zone"),
        ),
        Sample::new("RAW(16)", Some("varbinary")),
        Sample::new("BLOB", Some("varbinary")),
        Sample::new("FLOAT(126)", Some("double")),
        Sample::new("VARCHAR2(10)", None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{OracleSpec, PostgresSpec, SourceEngine};
    use crate::model::SourceColumn;
    use crate::typemap::Mapping;

    #[test]
    fn the_builtin_type_maps_pass_the_kit() {
        for (spec, samples) in [
            (&PostgresSpec as &dyn EngineSpec, postgres_samples()),
            (&OracleSpec as &dyn EngineSpec, oracle_samples()),
        ] {
            let r = check(spec, &samples);
            assert!(r.passed(), "{}: {:#?}", spec.engine(), r.failures);
            assert!(
                r.refused > 0,
                "{}: the samples include a refusal",
                spec.engine()
            );
            assert!(r.refused < r.checked);
        }
    }

    /// A map that drops a column — no type, not refused — fails the kit.
    struct Drops;

    impl EngineSpec for Drops {
        fn engine(&self) -> SourceEngine {
            SourceEngine::from_static("drops")
        }
        fn map_column(&self, c: &SourceColumn) -> Mapping {
            let mut m = crate::typemap::map_column(c);
            if c.pg_type == "text" {
                m.iceberg_type = String::new();
                m.expr = String::new();
            }
            if c.pg_type == "integer" {
                m.lossiness = Lossiness::NarrowingSafe;
            }
            m
        }
        fn orderable(&self, _: &str) -> bool {
            false
        }
    }

    #[test]
    fn a_map_that_drops_a_column_or_narrows_unmeasured_fails() {
        let r = check(
            &Drops,
            &[
                Sample::new("text", Some("varchar")),
                Sample::new("integer", Some("integer")),
                Sample::new("bigint", Some("bigint")),
            ],
        );
        assert!(!r.passed());
        assert_eq!(r.failures.len(), 2, "{:#?}", r.failures);
        assert!(r.failures[0].starts_with("text (Some(\"varchar\")): dropped"));
        assert!(r.failures[1].contains("narrowing_safe without a measurement"));
    }
}

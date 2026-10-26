//! RFC 0216 §2.4: (Oracle type, connector type) → Iceberg type, with a lossiness class.
//!
//! Decided from the Oracle type first, as [`crate::typemap`] does for PostgreSQL: the Trino
//! connector reads an unconstrained `NUMBER` as its own `number` type and a `DATE` as
//! `timestamp(0)`, and only Oracle's own type says what the column can hold.

use crate::model::SourceColumn;
use crate::typemap::{CompareRule, Lossiness, Mapping, ident};

/// The scale an unconstrained `NUMBER` with no values to measure lands at. A later value with
/// more digits is rounded by the load's cast and caught by the next reconciliation, whose Oracle
/// rendering of a decimal never rounds (EKOS `migrate-validate`, `Dialect::Oracle`).
pub const NO_VALUES_SCALE: u32 = 10;

/// Types the connector renders as text that Iceberg cannot hold natively.
const AS_TEXT: &[&str] = &["ROWID", "UROWID", "XMLTYPE"];

/// Never landed: the connector cannot read them faithfully, or Oracle cannot compare them.
const REFUSED: &[&str] = &["LONG", "LONG RAW", "BFILE"];

fn base(oracle_type: &str) -> &str {
    oracle_type.split('(').next().unwrap_or(oracle_type).trim()
}

/// `(p, s)` of `NUMBER(p,s)`.
/// `char(3)` → 3.
fn char_length(trino_type: &str) -> Option<u32> {
    trino_type
        .strip_prefix("char(")?
        .strip_suffix(')')?
        .trim()
        .parse()
        .ok()
}

fn number_params(t: &str) -> Option<(i64, i64)> {
    let inner = t.strip_prefix("NUMBER(")?.strip_suffix(')')?;
    let (p, s) = inner.split_once(',')?;
    Some((p.trim().parse().ok()?, s.trim().parse().ok()?))
}

/// The precision of `TIMESTAMP(p)…`.
fn timestamp_precision(t: &str) -> Option<u32> {
    let inner = t.strip_prefix("TIMESTAMP(")?;
    inner[..inner.find(')')?].trim().parse().ok()
}

fn set(
    mut m: Mapping,
    iceberg: &str,
    expr: Option<String>,
    lossiness: Lossiness,
    rule: &str,
    why: &str,
    compare: Option<CompareRule>,
) -> Mapping {
    if let Some(e) = expr {
        m.expr = e;
    }
    m.iceberg_type = iceberg.into();
    m.lossiness = lossiness;
    m.rule = rule.into();
    m.rationale = why.into();
    m.compare = compare;
    m
}

fn refuse(mut m: Mapping, rule: &str, why: String) -> Mapping {
    m.rule = rule.into();
    m.rationale = why;
    m.lossiness = Lossiness::Refused;
    m
}

pub fn map_column(col: &SourceColumn) -> Mapping {
    let id = ident(&col.name);
    let m = Mapping {
        column: col.name.clone(),
        pg_type: col.pg_type.clone(),
        trino_type: col.trino_type.clone(),
        iceberg_type: String::new(),
        expr: id.clone(),
        lossiness: Lossiness::Refused,
        rule: String::new(),
        rationale: String::new(),
        compare: None,
    };
    let t = col.pg_type.as_str();
    if REFUSED.contains(&t) {
        return refuse(
            m,
            "oracle-unsupported",
            format!("{t} has no faithful landing through the connector"),
        );
    }
    let Some(trino) = col.trino_type.clone() else {
        return refuse(
            m,
            "connector-missing",
            "the Trino connector does not expose this column; set \
             unsupported-type-handling=CONVERT_TO_VARCHAR on the source catalog"
                .into(),
        );
    };
    let cast = |to: &str| Some(format!("CAST({id} AS {to})"));
    match base(t) {
        "NUMBER" | "INTEGER" => number(m, t, col.numeric_used, &id),
        "FLOAT" | "BINARY_DOUBLE" => set(
            m,
            "double",
            cast("double"),
            Lossiness::Exact,
            "float",
            "binary floating point, landed as double; not compared",
            None,
        ),
        "BINARY_FLOAT" => set(
            m,
            "real",
            cast("real"),
            Lossiness::Exact,
            "float",
            "binary floating point, landed as real; not compared",
            None,
        ),
        "VARCHAR2" | "NVARCHAR2" => set(
            m,
            "varchar",
            None,
            Lossiness::Exact,
            "varchar",
            "Iceberg strings have no length; Oracle stores '' as NULL",
            Some(CompareRule::Text),
        ),
        // The padding is part of the value (RFC 0155), and Oracle returns it: iDempiere's
        // `UserLevel` is 'S  ' or ' CO'. Trino's cast from char(n) to varchar drops trailing
        // spaces, so the value is padded back to n (EKOS RFC 0227: found by reconciliation).
        // A CHAR(1) value has no padding to lose (Oracle's '' is NULL): its plain cast stays, so
        // the fix changed no flag column's expression and reloaded no table for it.
        "CHAR" | "NCHAR" => match char_length(&trino) {
            Some(1) => set(
                m,
                "varchar",
                cast("varchar"),
                Lossiness::AsText,
                "char-padded",
                "CHAR(1) landed as varchar",
                Some(CompareRule::Text),
            ),
            Some(n) => set(
                m,
                "varchar",
                Some(format!("rpad(CAST({id} AS varchar), {n}, ' ')")),
                Lossiness::AsText,
                "char-padded",
                "CHAR(n) landed as varchar with its padding",
                Some(CompareRule::Text),
            ),
            None => refuse(
                m,
                "char-length-unknown",
                format!(
                    "the connector types it {trino}: its length, and so its padding, is unknown"
                ),
            ),
        },
        "CLOB" | "NCLOB" => set(
            m,
            "varchar",
            None,
            Lossiness::Exact,
            "lob-text",
            "a LOB, landed as varchar; not compared (Oracle cannot hash a LOB in SQL without \
             DBMS_CRYPTO)",
            None,
        ),
        "DATE" => set(
            m,
            "timestamp(6)",
            cast("timestamp(6)"),
            Lossiness::Widening,
            "oracle-date",
            "an Oracle DATE holds a time of day to the second",
            Some(CompareRule::TimestampNaive),
        ),
        "TIMESTAMP" => timestamp(m, t, &id),
        "RAW" => set(
            m,
            "varbinary",
            None,
            Lossiness::Exact,
            "raw",
            "bytes",
            Some(CompareRule::Bytes),
        ),
        "BLOB" => set(
            m,
            "varbinary",
            None,
            Lossiness::Exact,
            "lob-bytes",
            "a LOB, landed as varbinary; not compared",
            None,
        ),
        b if (AS_TEXT.contains(&b) || b.starts_with("INTERVAL"))
            && trino.starts_with("varchar") =>
        {
            set(
                m,
                "varchar",
                None,
                Lossiness::AsText,
                "as-text",
                &format!("{b} has no Iceberg type; the connector's text rendering is kept"),
                Some(CompareRule::Text),
            )
        }
        other => refuse(
            m,
            "oracle-type",
            format!("{other} (connector: {trino}) has no landing yet"),
        ),
    }
}

fn number(m: Mapping, t: &str, measured: Option<(u32, u32)>, id: &str) -> Mapping {
    let cast = |to: &str| Some(format!("CAST({id} AS {to})"));
    if t == "INTEGER" {
        return set(
            m,
            "decimal(38,0)",
            cast("decimal(38,0)"),
            Lossiness::Exact,
            "number-integer",
            "INTEGER is NUMBER(38,0)",
            Some(CompareRule::Decimal(0)),
        );
    }
    match number_params(t) {
        Some((_, s)) if s < 0 => refuse(
            m,
            "number-negative-scale",
            format!("{t} rounds to the left of the point; no exact decimal landing"),
        ),
        Some((p, 0)) if p <= 9 => set(
            m,
            "integer",
            cast("integer"),
            Lossiness::Exact,
            "number-int",
            &format!("{t} holds at most 9 digits"),
            Some(CompareRule::Int),
        ),
        Some((p, 0)) if p <= 18 => set(
            m,
            "bigint",
            cast("bigint"),
            Lossiness::Exact,
            "number-bigint",
            &format!("{t} holds at most 18 digits"),
            Some(CompareRule::Int),
        ),
        Some((p, s)) if s <= p && p <= 38 => {
            let d = format!("decimal({p},{s})");
            let e = cast(&d);
            set(
                m,
                &d,
                e,
                Lossiness::Exact,
                "number-decimal",
                "the same precision and scale",
                Some(CompareRule::Decimal(s as u32)),
            )
        }
        Some(_) => refuse(
            m,
            "number-scale",
            format!("{t}: a scale above the precision has no Iceberg decimal"),
        ),
        None => match measured {
            Some((0, s)) => {
                let d = format!("decimal(38,{s})");
                let e = cast(&d);
                set(
                    m,
                    &d,
                    e,
                    Lossiness::NarrowingSafe,
                    "number-no-values",
                    &format!(
                        "unconstrained NUMBER with no values to measure; landed at scale {s}, and \
                         a value with more digits fails the next reconciliation"
                    ),
                    Some(CompareRule::Decimal(s)),
                )
            }
            Some((p, s)) if p <= 38 => {
                let d = format!("decimal(38,{s})");
                let e = cast(&d);
                set(
                    m,
                    &d,
                    e,
                    Lossiness::NarrowingSafe,
                    "number-measured",
                    &format!("unconstrained NUMBER; the data uses precision {p}, scale {s}"),
                    Some(CompareRule::Decimal(s)),
                )
            }
            Some((p, s)) => refuse(
                m,
                "number-too-precise",
                format!(
                    "unconstrained NUMBER whose data uses precision {p}, scale {s}: more digits \
                     than Iceberg's 38 (a stored division result?)"
                ),
            ),
            None => refuse(
                m,
                "number-unbounded",
                "unconstrained NUMBER with no measured precision and scale: Iceberg has no \
                 unbounded decimal (measured only for tables loaded in full)"
                    .into(),
            ),
        },
    }
}

fn timestamp(m: Mapping, t: &str, id: &str) -> Mapping {
    let zoned = t.ends_with("TIME ZONE");
    match timestamp_precision(t) {
        Some(p) if p > 6 => refuse(
            m,
            "timestamp-precision",
            format!("{t}: Iceberg keeps microseconds; sub-microsecond digits would be cut"),
        ),
        _ if zoned => set(
            m,
            "timestamp(6) with time zone",
            Some(format!("CAST({id} AS timestamp(6) with time zone)")),
            Lossiness::Exact,
            "timestamptz",
            "an instant, at microseconds",
            Some(CompareRule::TimestampUtc),
        ),
        p => set(
            m,
            "timestamp(6)",
            Some(format!("CAST({id} AS timestamp(6))")),
            if p == Some(6) {
                Lossiness::Exact
            } else {
                Lossiness::Widening
            },
            "timestamp",
            "a local timestamp, at microseconds",
            Some(CompareRule::TimestampNaive),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TypeKind;

    fn col(oracle: &str, trino: Option<&str>) -> SourceColumn {
        SourceColumn {
            name: "C".into(),
            pg_type: oracle.into(),
            kind: TypeKind::Base,
            not_null: false,
            trino_type: trino.map(str::to_string),
            numeric_used: None,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
            attnum: 1,
            has_default: false,
        }
    }

    fn mapped(oracle: &str, trino: &str) -> Mapping {
        map_column(&col(oracle, Some(trino)))
    }

    #[test]
    fn integer_numbers_land_as_integers_by_precision() {
        let m = mapped("NUMBER(10,0)", "decimal(10,0)");
        assert_eq!(
            (m.iceberg_type.as_str(), m.lossiness),
            ("bigint", Lossiness::Exact)
        );
        assert_eq!(m.expr, "CAST(\"C\" AS bigint)");
        assert_eq!(m.compare, Some(CompareRule::Int));
        assert_eq!(
            mapped("NUMBER(9,0)", "decimal(9,0)").iceberg_type,
            "integer"
        );
        let wide = mapped("NUMBER(20,0)", "decimal(20,0)");
        assert_eq!(wide.iceberg_type, "decimal(20,0)");
        assert_eq!(wide.compare, Some(CompareRule::Decimal(0)));
        assert_eq!(
            mapped("INTEGER", "decimal(38,0)").iceberg_type,
            "decimal(38,0)"
        );
    }

    #[test]
    fn a_decimal_keeps_its_precision_and_scale() {
        let m = mapped("NUMBER(22,2)", "decimal(22,2)");
        assert_eq!(m.iceberg_type, "decimal(22,2)");
        assert_eq!(m.compare, Some(CompareRule::Decimal(2)));
        assert_eq!(
            mapped("NUMBER(5,-2)", "decimal(7,0)").lossiness,
            Lossiness::Refused
        );
    }

    #[test]
    fn an_unconstrained_number_needs_a_measurement() {
        let mut c = col("NUMBER", Some("number"));
        let refused = map_column(&c);
        assert_eq!(refused.lossiness, Lossiness::Refused);
        assert!(refused.rationale.contains("no measured"), "{refused:?}");
        c.numeric_used = Some((12, 4));
        let m = map_column(&c);
        assert_eq!(
            (m.iceberg_type.as_str(), m.lossiness),
            ("decimal(38,4)", Lossiness::NarrowingSafe)
        );
        assert_eq!(m.compare, Some(CompareRule::Decimal(4)));
        c.numeric_used = Some((40, 2));
        assert_eq!(map_column(&c).rule, "number-too-precise");
        c.numeric_used = Some((0, NO_VALUES_SCALE));
        let empty = map_column(&c);
        assert_eq!(
            (empty.iceberg_type.as_str(), empty.rule.as_str()),
            ("decimal(38,10)", "number-no-values")
        );
    }

    #[test]
    fn text_types() {
        let v = mapped("VARCHAR2(60 CHAR)", "varchar(60)");
        assert_eq!(
            (v.iceberg_type.as_str(), v.lossiness),
            ("varchar", Lossiness::Exact)
        );
        assert_eq!(v.expr, "\"C\"");
        let c = mapped("CHAR(1)", "char(1)");
        assert_eq!(
            (c.lossiness, c.compare),
            (Lossiness::AsText, Some(CompareRule::Text))
        );
        // One character has no padding to lose; wider, Trino's cast drops trailing spaces and
        // the padding is put back.
        assert_eq!(c.expr, "CAST(\"C\" AS varchar)");
        assert_eq!(
            mapped("CHAR(3)", "char(3)").expr,
            "rpad(CAST(\"C\" AS varchar), 3, ' ')"
        );
        assert_eq!(mapped("CHAR(3)", "varchar").lossiness, Lossiness::Refused);
        let lob = mapped("CLOB", "varchar");
        assert_eq!((lob.lossiness, lob.compare), (Lossiness::Exact, None));
    }

    #[test]
    fn dates_and_timestamps() {
        let d = mapped("DATE", "timestamp(0)");
        assert_eq!(
            (d.iceberg_type.as_str(), d.lossiness, d.compare),
            (
                "timestamp(6)",
                Lossiness::Widening,
                Some(CompareRule::TimestampNaive)
            )
        );
        assert_eq!(
            mapped("TIMESTAMP(6)", "timestamp(6)").lossiness,
            Lossiness::Exact
        );
        assert_eq!(
            mapped("TIMESTAMP(3)", "timestamp(3)").lossiness,
            Lossiness::Widening
        );
        assert_eq!(
            mapped("TIMESTAMP(9)", "timestamp(9)").lossiness,
            Lossiness::Refused
        );
        let z = mapped("TIMESTAMP(6) WITH TIME ZONE", "timestamp(6) with time zone");
        assert_eq!(z.compare, Some(CompareRule::TimestampUtc));
        assert_eq!(z.iceberg_type, "timestamp(6) with time zone");
    }

    #[test]
    fn bytes_floats_and_the_rest() {
        assert_eq!(
            mapped("RAW(16)", "varbinary").compare,
            Some(CompareRule::Bytes)
        );
        assert_eq!(mapped("BLOB", "varbinary").compare, None);
        let f = mapped("BINARY_DOUBLE", "double");
        assert_eq!((f.iceberg_type.as_str(), f.compare), ("double", None));
        assert_eq!(mapped("ROWID", "varchar").lossiness, Lossiness::AsText);
        assert_eq!(
            mapped("INTERVAL DAY(2) TO SECOND(6)", "varchar").lossiness,
            Lossiness::AsText
        );
        assert_eq!(mapped("LONG", "varchar").lossiness, Lossiness::Refused);
        assert_eq!(
            mapped("SDO_GEOMETRY", "varchar").lossiness,
            Lossiness::Refused
        );
        let missing = map_column(&col("VARCHAR2(10)", None));
        assert_eq!(missing.rule, "connector-missing");
    }
}

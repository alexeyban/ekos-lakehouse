//! RFC 0192 §2: (PostgreSQL type, connector type) → Iceberg type, with a lossiness class.
//!
//! Decided from the PostgreSQL type first and the connector's type second: with
//! `unsupported-type-handling=CONVERT_TO_VARCHAR` a domain, a `tsvector` and an `interval` all
//! arrive as `varchar`, and only PostgreSQL knows which of them is an integer underneath.

use crate::model::{SourceColumn, TypeKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lossiness {
    /// Same values in the same type family.
    Exact,
    /// Every value fits a wider type.
    Widening,
    /// PostgreSQL's text rendering, kept exactly; the type's semantics are not.
    AsText,
    /// Narrower than the declared type, wide enough for every measured value.
    NarrowingSafe,
    /// No faithful landing: the column refuses its table.
    Refused,
}

/// The canonical-form rule (RFC 0155) both sides of a reconciliation render this column with.
/// `None` on [`Mapping::compare`] means the column is not compared, and is reported as such.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "rule", content = "scale")]
pub enum CompareRule {
    Text,
    Bool,
    Int,
    Decimal(u32),
    TimestampUtc,
    TimestampNaive,
    Date,
    Time,
    Uuid,
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mapping {
    pub column: String,
    pub pg_type: String,
    pub trino_type: Option<String>,
    /// The Iceberg column type, in Trino's spelling. Empty when refused.
    pub iceberg_type: String,
    /// The `SELECT` expression over the connector's column.
    pub expr: String,
    pub lossiness: Lossiness,
    pub rule: String,
    pub rationale: String,
    pub compare: Option<CompareRule>,
}

/// Quote an identifier for Trino. Never concatenate a raw one.
pub fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// PostgreSQL types whose connector `varchar` is their text rendering (`as_text`).
const AS_TEXT: &[&str] = &[
    "tsvector",
    "tsquery",
    "interval",
    "inet",
    "cidr",
    "macaddr",
    "macaddr8",
    "money",
    "xml",
    "json",
    "jsonb",
    "citext",
    "point",
    "line",
    "lseg",
    "box",
    "path",
    "polygon",
    "circle",
    "oid",
    "pg_lsn",
    "txid_snapshot",
];

fn base_name(pg_type: &str) -> &str {
    pg_type.split('(').next().unwrap_or(pg_type).trim()
}

pub fn map_column(col: &SourceColumn) -> Mapping {
    let id = ident(&col.name);
    let mut m = Mapping {
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
    let Some(trino) = col.trino_type.as_deref() else {
        m.rule = "connector-missing".into();
        m.rationale = "the Trino connector does not expose this column; set \
                       postgresql.array-mapping=AS_ARRAY and \
                       unsupported-type-handling=CONVERT_TO_VARCHAR on the source catalog"
            .into();
        return m;
    };

    // A domain arrives as varchar under CONVERT_TO_VARCHAR; land it as its base type.
    if let TypeKind::Domain { base } = &col.kind {
        let inner = map_column(&SourceColumn {
            name: col.name.clone(),
            pg_type: base.clone(),
            kind: TypeKind::Base,
            not_null: col.not_null,
            trino_type: expected_trino_type(base),
            numeric_used: col.numeric_used,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
        });
        if inner.lossiness == Lossiness::Refused {
            return Mapping {
                pg_type: col.pg_type.clone(),
                trino_type: col.trino_type.clone(),
                rule: format!("domain:{}", inner.rule),
                ..inner
            };
        }
        let from_text = trino.starts_with("varchar");
        return Mapping {
            pg_type: col.pg_type.clone(),
            trino_type: col.trino_type.clone(),
            expr: if from_text {
                format!("CAST({id} AS {})", inner.iceberg_type)
            } else {
                inner.expr
            },
            rule: "domain-base".into(),
            rationale: format!("domain over {base}, landed as its base type"),
            ..inner
        };
    }

    if let Some(n) = col.infinite_values.filter(|n| *n > 0) {
        m.rule = "infinite-timestamp".into();
        m.rationale = format!(
            "{n} row(s) hold infinity or -infinity, which the Trino connector reads as a wrong \
             finite value (measured: 'infinity' read as year -171979); land it through a source \
             view that maps infinity explicitly"
        );
        return m;
    }
    let pg_base = base_name(&col.pg_type);
    if col.kind == TypeKind::Enum && trino.starts_with("varchar") {
        return exact(
            m,
            "varchar",
            "enum-label",
            "an enum's labels are its values",
            Some(CompareRule::Text),
        );
    }
    if pg_base == "vector" && trino == "array(real)" {
        return exact(
            m,
            "array(real)",
            "pgvector",
            "pgvector's elements are real",
            None,
        );
    }
    if AS_TEXT.contains(&pg_base) || (col.kind == TypeKind::Other && trino.starts_with("varchar")) {
        if trino == "json" {
            m.expr = format!("json_format({id})");
        }
        m.iceberg_type = "varchar".into();
        m.lossiness = Lossiness::AsText;
        m.rule = "as-text".into();
        m.rationale = format!("{pg_base} has no Iceberg type; PostgreSQL's text rendering is kept");
        m.compare = Some(CompareRule::Text);
        return m;
    }
    if pg_base == "numeric" && !col.pg_type.contains('(') {
        return match col.numeric_used {
            Some((p, s)) if p <= 38 => {
                let t = format!("decimal(38,{s})");
                m.expr = format!("CAST({id} AS {t})");
                m.iceberg_type = t;
                m.lossiness = Lossiness::NarrowingSafe;
                m.rule = "numeric-measured".into();
                m.rationale =
                    format!("unconstrained numeric; the profile measured precision {p}, scale {s}");
                m.compare = Some(CompareRule::Decimal(s));
                m
            }
            _ => {
                m.rule = "numeric-unbounded".into();
                m.rationale = "unconstrained numeric with no measured precision and scale: \
                               Iceberg has no unbounded decimal (profile it at P1)"
                    .into();
                m
            }
        };
    }
    map_trino(m, trino)
}

fn exact(
    mut m: Mapping,
    iceberg: &str,
    rule: &str,
    why: &str,
    compare: Option<CompareRule>,
) -> Mapping {
    m.iceberg_type = iceberg.into();
    m.lossiness = Lossiness::Exact;
    m.rule = rule.into();
    m.rationale = why.into();
    m.compare = compare;
    m
}

/// By the connector's type, for a column PostgreSQL did not single out above.
fn map_trino(mut m: Mapping, trino: &str) -> Mapping {
    let id = ident(&m.column);
    let same =
        |m: Mapping, t: &str, c: Option<CompareRule>| exact(m, t, "same", "the same type", c);
    match trino {
        "boolean" => same(m, "boolean", Some(CompareRule::Bool)),
        "integer" | "bigint" => same(m, trino, Some(CompareRule::Int)),
        "real" | "double" => same(m, trino, None),
        "date" => same(m, "date", Some(CompareRule::Date)),
        "uuid" => same(m, "uuid", Some(CompareRule::Uuid)),
        "varbinary" => same(m, "varbinary", Some(CompareRule::Bytes)),
        "smallint" | "tinyint" => {
            m.expr = format!("CAST({id} AS integer)");
            m.iceberg_type = "integer".into();
            m.lossiness = Lossiness::Widening;
            m.rule = "int-widen".into();
            m.rationale = format!("Iceberg has no {trino}");
            m.compare = Some(CompareRule::Int);
            m
        }
        t if t.starts_with("varchar") => exact(
            m,
            "varchar",
            "varchar",
            "Iceberg strings have no length",
            Some(CompareRule::Text),
        ),
        t if t.starts_with("char(") => {
            // Iceberg has no char. The padding is part of the value (RFC 0155): keep it.
            m.expr = format!("CAST({id} AS varchar)");
            exact(
                m,
                "varchar",
                "char-padded",
                "char(n) landed as varchar with its padding",
                Some(CompareRule::Text),
            )
        }
        t if t.starts_with("decimal(") => match decimal_params(t) {
            Some((p, s)) if p <= 38 => same(m, t, Some(CompareRule::Decimal(s))),
            _ => {
                m.rule = "decimal-too-wide".into();
                m.rationale = format!("{t} exceeds Iceberg's 38 digits");
                m
            }
        },
        t if t.starts_with("time(") && !t.contains("zone") => {
            m.expr = format!("CAST({id} AS time(6))");
            exact(
                m,
                "time(6)",
                "time",
                "time at microseconds",
                Some(CompareRule::Time),
            )
        }
        t if t.starts_with("timestamp(") && t.ends_with("with time zone") => {
            m.expr = format!("CAST({id} AS timestamp(6) with time zone)");
            exact(
                m,
                "timestamp(6) with time zone",
                "timestamptz",
                "an instant, at microseconds",
                Some(CompareRule::TimestampUtc),
            )
        }
        t if t.starts_with("timestamp(") => {
            m.expr = format!("CAST({id} AS timestamp(6))");
            exact(
                m,
                "timestamp(6)",
                "timestamp",
                "a local timestamp, at microseconds",
                Some(CompareRule::TimestampNaive),
            )
        }
        t if t.starts_with("array(") => {
            // Arrays of types Iceberg holds as they are; no canonical rule compares them yet.
            let inner = &t["array(".len()..t.len() - 1];
            if matches!(
                inner,
                "integer" | "bigint" | "boolean" | "real" | "double" | "date" | "uuid"
            ) || inner.starts_with("varchar")
                || inner.starts_with("decimal(")
            {
                let elem = if inner.starts_with("varchar") {
                    "varchar"
                } else {
                    inner
                };
                let t = format!("array({elem})");
                m.expr = format!("CAST({id} AS {t})");
                exact(m, &t, "array", "an array of a type Iceberg holds", None)
            } else {
                m.rule = "array-element".into();
                m.rationale = format!("no landing for arrays of {inner} yet");
                m
            }
        }
        other => {
            m.rule = "unmapped".into();
            m.rationale = format!("no Iceberg landing for connector type {other}");
            m
        }
    }
}

fn decimal_params(t: &str) -> Option<(u32, u32)> {
    let inner = t.strip_prefix("decimal(")?.strip_suffix(')')?;
    let (p, s) = inner.split_once(',')?;
    Some((p.trim().parse().ok()?, s.trim().parse().ok()?))
}

/// What the connector exposes for a PostgreSQL base type, for a domain's base.
pub fn expected_trino_type(pg: &str) -> Option<String> {
    let b = base_name(pg);
    Some(match b {
        "boolean" => "boolean".into(),
        "smallint" => "smallint".into(),
        "integer" => "integer".into(),
        "bigint" => "bigint".into(),
        "real" => "real".into(),
        "double precision" => "double".into(),
        "text" | "character varying" => "varchar".into(),
        "date" => "date".into(),
        "uuid" => "uuid".into(),
        "bytea" => "varbinary".into(),
        "numeric" if pg.contains('(') => {
            format!("decimal{}", &pg[pg.find('(')?..])
        }
        "timestamp with time zone" => "timestamp(6) with time zone".into(),
        "timestamp without time zone" => "timestamp(6)".into(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, pg: &str, kind: TypeKind, trino: Option<&str>) -> SourceColumn {
        SourceColumn {
            name: name.into(),
            pg_type: pg.into(),
            kind,
            not_null: false,
            trino_type: trino.map(str::to_string),
            numeric_used: None,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
        }
    }

    fn m(pg: &str, kind: TypeKind, trino: &str) -> Mapping {
        map_column(&col("c", pg, kind, Some(trino)))
    }

    /// The Pagila measurement of 2026-10-16 (RFC 0192), row by row.
    #[test]
    fn pagila_types_land_as_measured() {
        use Lossiness::*;
        // (pg type, kind, connector type, iceberg type, lossiness, compare rule)
        type Case<'a> = (
            &'a str,
            TypeKind,
            &'a str,
            &'a str,
            Lossiness,
            Option<CompareRule>,
        );
        let cases: &[Case] = &[
            (
                "integer",
                TypeKind::Base,
                "integer",
                "integer",
                Exact,
                Some(CompareRule::Int),
            ),
            (
                "smallint",
                TypeKind::Base,
                "smallint",
                "integer",
                Widening,
                Some(CompareRule::Int),
            ),
            (
                "text",
                TypeKind::Base,
                "varchar",
                "varchar",
                Exact,
                Some(CompareRule::Text),
            ),
            (
                "numeric(5,2)",
                TypeKind::Base,
                "decimal(5,2)",
                "decimal(5,2)",
                Exact,
                Some(CompareRule::Decimal(2)),
            ),
            (
                "timestamp with time zone",
                TypeKind::Base,
                "timestamp(6) with time zone",
                "timestamp(6) with time zone",
                Exact,
                Some(CompareRule::TimestampUtc),
            ),
            (
                "uuid",
                TypeKind::Base,
                "uuid",
                "uuid",
                Exact,
                Some(CompareRule::Uuid),
            ),
            (
                "boolean",
                TypeKind::Base,
                "boolean",
                "boolean",
                Exact,
                Some(CompareRule::Bool),
            ),
            (
                "date",
                TypeKind::Base,
                "date",
                "date",
                Exact,
                Some(CompareRule::Date),
            ),
            (
                "bytea",
                TypeKind::Base,
                "varbinary",
                "varbinary",
                Exact,
                Some(CompareRule::Bytes),
            ),
            (
                "mpaa_rating",
                TypeKind::Enum,
                "varchar",
                "varchar",
                Exact,
                Some(CompareRule::Text),
            ),
            (
                "vector(20)",
                TypeKind::Base,
                "array(real)",
                "array(real)",
                Exact,
                None,
            ),
            (
                "text[]",
                TypeKind::Array {
                    element: "text".into(),
                },
                "array(varchar)",
                "array(varchar)",
                Exact,
                None,
            ),
            (
                "tsvector",
                TypeKind::Base,
                "varchar",
                "varchar",
                AsText,
                Some(CompareRule::Text),
            ),
            (
                "year",
                TypeKind::Domain {
                    base: "integer".into(),
                },
                "varchar",
                "integer",
                Exact,
                Some(CompareRule::Int),
            ),
        ];
        for (pg, kind, trino, iceberg, loss, cmp) in cases {
            let got = m(pg, kind.clone(), trino);
            assert_eq!(
                (got.iceberg_type.as_str(), got.lossiness, got.compare),
                (*iceberg, *loss, *cmp),
                "{pg} via {trino}: {got:?}"
            );
        }
    }

    #[test]
    fn a_domain_from_text_is_cast_back_to_its_base() {
        let got = m(
            "year",
            TypeKind::Domain {
                base: "integer".into(),
            },
            "varchar",
        );
        assert_eq!(got.expr, "CAST(\"c\" AS integer)");
        assert_eq!(got.rule, "domain-base");
    }

    #[test]
    fn a_column_the_connector_hides_is_refused_with_the_fix() {
        let got = map_column(&col(
            "c",
            "text[]",
            TypeKind::Array {
                element: "text".into(),
            },
            None,
        ));
        assert_eq!(got.lossiness, Lossiness::Refused);
        assert!(got.rationale.contains("AS_ARRAY"), "{}", got.rationale);
    }

    #[test]
    fn unconstrained_numeric_needs_a_measurement() {
        let mut c = col("amt", "numeric", TypeKind::Base, Some("number"));
        assert_eq!(map_column(&c).lossiness, Lossiness::Refused);
        c.numeric_used = Some((14, 4));
        let got = map_column(&c);
        assert_eq!(got.lossiness, Lossiness::NarrowingSafe);
        assert_eq!(got.iceberg_type, "decimal(38,4)");
        assert_eq!(got.compare, Some(CompareRule::Decimal(4)));
        c.numeric_used = Some((40, 2));
        assert_eq!(map_column(&c).lossiness, Lossiness::Refused);
    }

    #[test]
    fn wide_decimals_and_unknown_types_are_refused() {
        assert_eq!(
            m("numeric(40,2)", TypeKind::Base, "decimal(40,2)").lossiness,
            Lossiness::Refused
        );
        assert_eq!(
            m("weird", TypeKind::Base, "row(a integer)").lossiness,
            Lossiness::Refused
        );
    }

    #[test]
    fn a_timestamp_holding_infinity_is_refused() {
        let mut c = col("validto", "timestamp without time zone", TypeKind::Base, Some("timestamp(6)"));
        c.infinite_values = Some(1);
        let got = map_column(&c);
        assert_eq!((got.lossiness, got.rule.as_str()), (Lossiness::Refused, "infinite-timestamp"));
        c.infinite_values = Some(0);
        assert_eq!(map_column(&c).lossiness, Lossiness::Exact);
    }

    #[test]
    fn json_is_kept_as_its_text() {
        let got = m("jsonb", TypeKind::Base, "json");
        assert_eq!(
            (got.lossiness, got.expr.as_str()),
            (Lossiness::AsText, "json_format(\"c\")")
        );
    }

    #[test]
    fn timestamps_and_char_keep_their_precision_and_padding() {
        assert_eq!(
            m(
                "timestamp(3) without time zone",
                TypeKind::Base,
                "timestamp(3)"
            )
            .expr,
            "CAST(\"c\" AS timestamp(6))"
        );
        let ch = m("character(20)", TypeKind::Base, "char(20)");
        assert_eq!(
            (ch.iceberg_type.as_str(), ch.rule.as_str()),
            ("varchar", "char-padded")
        );
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(ident("we\"ird"), "\"we\"\"ird\"");
    }
}

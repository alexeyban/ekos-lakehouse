//! A Table input's PostgreSQL rewritten for Trino (RFC 0204) by a whitelist walker: only the
//! constructs whose meaning is the same in both engines pass; a few are rewritten (`::` casts,
//! `ILIKE`, PostgreSQL type names, `SUBSTRING … FROM … FOR`); anything else stops with its
//! name. Tables become `{{ ref('…') }}`.

use ekos_lakehouse_kettle_expr::Gap;
use sqlparser::ast::{
    BinaryOperator, CastKind, DataType, DateTimeField, ExactNumberInfo, Expr, Function,
    FunctionArg, FunctionArgExpr, FunctionArgumentList, FunctionArguments, Ident, ObjectName,
    Query, SetExpr, Statement, TableAlias, TableFactor, UnaryOperator, VisitMut, VisitorMut,
};
use sqlparser::dialect::PostgreSqlDialect;
use std::ops::ControlFlow;

/// Functions whose meaning is the same in PostgreSQL and Trino (for the argument types a
/// Table input passes them).
const FUNCTIONS: &[&str] = &[
    "coalesce",
    "nullif",
    "upper",
    "lower",
    "trim",
    "ltrim",
    "rtrim",
    "length",
    "replace",
    "substr",
    "abs",
    "round",
    "ceil",
    "ceiling",
    "floor",
    "sqrt",
    "power",
    "mod",
    "greatest",
    "least",
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "concat",
    "date_trunc",
    "now",
    "lpad",
    "rpad",
    "strpos",
    "row_number",
    "rank",
    "dense_rank",
    "lag",
    "lead",
    "first_value",
    "last_value",
    "stddev_pop",
    "stddev_samp",
    "var_pop",
    "var_samp",
    "bool_and",
    "bool_or",
    "sign",
    "exp",
    "ln",
    "reverse",
];

const OPERATORS: &[BinaryOperator] = &[
    BinaryOperator::Plus,
    BinaryOperator::Minus,
    BinaryOperator::Multiply,
    BinaryOperator::Divide,
    BinaryOperator::Modulo,
    BinaryOperator::Eq,
    BinaryOperator::NotEq,
    BinaryOperator::Lt,
    BinaryOperator::LtEq,
    BinaryOperator::Gt,
    BinaryOperator::GtEq,
    BinaryOperator::And,
    BinaryOperator::Or,
    BinaryOperator::StringConcat,
];

/// The result of a rewrite.
#[derive(Debug, Clone, PartialEq)]
pub struct Rewritten {
    /// Trino SQL with `{{ ref('model') }}` for each table.
    pub sql: String,
    /// Each rewrite applied, in words.
    pub notes: Vec<String>,
}

struct Walker<'a> {
    resolve: &'a dyn Fn(&ObjectName) -> Result<String, Gap>,
    refs: Vec<String>,
    gap: Option<Gap>,
    notes: Vec<String>,
}

impl Walker<'_> {
    fn stop(&mut self, what: impl Into<String>) -> ControlFlow<()> {
        if self.gap.is_none() {
            self.gap = Some(Gap::new(what));
        }
        ControlFlow::Break(())
    }
    fn note(&mut self, n: &str) {
        if !self.notes.iter().any(|x| x == n) {
            self.notes.push(n.to_string());
        }
    }
}

fn call(name: &str, arg: Expr) -> Expr {
    Expr::Function(Function {
        name: ObjectName(vec![Ident::new(name)]),
        uses_odbc_syntax: false,
        parameters: FunctionArguments::None,
        args: FunctionArguments::List(FunctionArgumentList {
            duplicate_treatment: None,
            args: vec![FunctionArg::Unnamed(FunctionArgExpr::Expr(arg))],
            clauses: vec![],
        }),
        filter: None,
        null_treatment: None,
        over: None,
        within_group: vec![],
    })
}

fn trino_type(t: &DataType, notes: &mut Vec<String>) -> Result<DataType, Gap> {
    use DataType as D;
    Ok(match t {
        D::Text | D::String(_) | D::Varchar(_) | D::CharVarying(_) | D::CharacterVarying(_) => {
            D::Varchar(None)
        }
        D::Char(n) | D::Character(n) => D::Char(*n),
        D::Int(_) | D::Integer(_) | D::Int4(_) => D::Integer(None),
        D::BigInt(_) | D::Int8(_) => D::BigInt(None),
        D::SmallInt(_) | D::Int2(_) => D::SmallInt(None),
        D::Numeric(ExactNumberInfo::None) | D::Decimal(ExactNumberInfo::None) => {
            let n = "an unconstrained numeric is cast as decimal(38, 10)".to_string();
            if !notes.contains(&n) {
                notes.push(n);
            }
            D::Decimal(ExactNumberInfo::PrecisionAndScale(38, 10))
        }
        D::Numeric(i) | D::Decimal(i) => D::Decimal(*i),
        D::Real | D::Float4 => D::Real,
        D::Float(_) | D::Double | D::DoublePrecision | D::Float8 => D::Double,
        D::Boolean | D::Bool => D::Boolean,
        D::Date => D::Date,
        D::Timestamp(p, tz) => D::Timestamp(*p, *tz),
        D::Bytea => D::Varbinary(None),
        D::Uuid => D::Uuid,
        other => return Err(Gap::new(format!("cast to {other}"))),
    })
}

impl VisitorMut for Walker<'_> {
    type Break = ();

    fn pre_visit_query(&mut self, q: &mut Query) -> ControlFlow<()> {
        if !q.locks.is_empty() {
            return self.stop("FOR UPDATE / FOR SHARE");
        }
        if let SetExpr::Select(s) = q.body.as_mut()
            && matches!(s.distinct, Some(sqlparser::ast::Distinct::On(_)))
        {
            return self.stop("DISTINCT ON");
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, f: &mut TableFactor) -> ControlFlow<()> {
        match f {
            TableFactor::Table {
                name,
                alias,
                args,
                with_hints,
                ..
            } => {
                if args.is_some() {
                    return self.stop(format!("table function {name}"));
                }
                if !with_hints.is_empty() {
                    return self.stop("table hints");
                }
                let model = match (self.resolve)(name) {
                    Ok(m) => m,
                    Err(g) => {
                        self.gap = Some(g);
                        return ControlFlow::Break(());
                    }
                };
                if alias.is_none() {
                    // Keep `orders.id` valid once `orders` is a ref.
                    let last = name.0.last().cloned().unwrap_or_else(|| Ident::new("t"));
                    *alias = Some(TableAlias {
                        name: last,
                        columns: vec![],
                    });
                }
                let placeholder = format!("ekos_ref_{}", self.refs.len());
                self.refs.push(model);
                *name = ObjectName(vec![Ident::new(placeholder)]);
                ControlFlow::Continue(())
            }
            TableFactor::Derived { lateral: true, .. } => self.stop("LATERAL"),
            TableFactor::Derived { .. } | TableFactor::NestedJoin { .. } => {
                ControlFlow::Continue(())
            }
            other => self.stop(format!("FROM item `{other}`")),
        }
    }

    fn pre_visit_expr(&mut self, e: &mut Expr) -> ControlFlow<()> {
        match e {
            Expr::Identifier(_)
            | Expr::CompoundIdentifier(_)
            | Expr::Value(_)
            | Expr::Nested(_)
            | Expr::IsNull(_)
            | Expr::IsNotNull(_)
            | Expr::IsTrue(_)
            | Expr::IsFalse(_)
            | Expr::IsNotTrue(_)
            | Expr::IsNotFalse(_)
            | Expr::IsDistinctFrom(..)
            | Expr::IsNotDistinctFrom(..)
            | Expr::Between { .. }
            | Expr::InList { .. }
            | Expr::InSubquery { .. }
            | Expr::Case { .. }
            | Expr::Subquery(_)
            | Expr::Exists { .. }
            | Expr::Tuple(_)
            | Expr::TypedString { .. } => ControlFlow::Continue(()),
            Expr::Like {
                escape_char: None,
                any: false,
                ..
            } => ControlFlow::Continue(()),
            Expr::BinaryOp { op, .. } => {
                if OPERATORS.contains(op) {
                    ControlFlow::Continue(())
                } else {
                    self.stop(format!("operator `{op}`"))
                }
            }
            Expr::UnaryOp { op, .. } => match op {
                UnaryOperator::Not | UnaryOperator::Minus | UnaryOperator::Plus => {
                    ControlFlow::Continue(())
                }
                other => self.stop(format!("operator `{other}`")),
            },
            Expr::ILike {
                negated,
                any: false,
                expr,
                pattern,
                escape_char: None,
            } => {
                self.note("ILIKE written as lower(…) LIKE lower(…)");
                let l = call("lower", (**expr).clone());
                let r = call("lower", (**pattern).clone());
                *e = Expr::Like {
                    negated: *negated,
                    any: false,
                    expr: Box::new(l),
                    pattern: Box::new(r),
                    escape_char: None,
                };
                ControlFlow::Continue(())
            }
            Expr::Cast {
                kind,
                data_type,
                format: None,
                ..
            } => {
                match kind {
                    CastKind::DoubleColon => {
                        self.note("`::` casts written as CAST");
                        *kind = CastKind::Cast;
                    }
                    CastKind::Cast | CastKind::TryCast => {}
                    CastKind::SafeCast => return self.stop("SAFE_CAST"),
                }
                match trino_type(data_type, &mut self.notes) {
                    Ok(t) => {
                        *data_type = t;
                        ControlFlow::Continue(())
                    }
                    Err(g) => self.stop(g.reason),
                }
            }
            Expr::Extract { field, .. } => match field {
                DateTimeField::Year
                | DateTimeField::Quarter
                | DateTimeField::Month
                | DateTimeField::Day
                | DateTimeField::Hour
                | DateTimeField::Minute
                | DateTimeField::Second => ControlFlow::Continue(()),
                other => self.stop(format!(
                    "EXTRACT({other}) (the engines number it differently)"
                )),
            },
            Expr::Substring { special, .. } => {
                *special = true;
                ControlFlow::Continue(())
            }
            Expr::Function(f) => {
                let name = f.name.to_string().to_ascii_lowercase();
                if f.filter.is_some() || !f.within_group.is_empty() {
                    return self.stop(format!("{name}(…) with FILTER / WITHIN GROUP"));
                }
                if FUNCTIONS.contains(&name.as_str()) {
                    ControlFlow::Continue(())
                } else {
                    self.stop(format!(
                        "function {name}(…) has no checked Trino equivalent"
                    ))
                }
            }
            other => {
                let text = other.to_string();
                let short: String = text.chars().take(60).collect();
                self.stop(format!("the expression `{short}`"))
            }
        }
    }
}

/// Rewrites `sql` (PostgreSQL) for Trino. `resolve` turns each table into a dbt model name.
pub fn to_trino(
    sql: &str,
    resolve: &dyn Fn(&ObjectName) -> Result<String, Gap>,
) -> Result<Rewritten, Gap> {
    let mut stmts = sqlparser::parser::Parser::parse_sql(&PostgreSqlDialect {}, sql)
        .map_err(|e| Gap::new(format!("the SQL does not parse as PostgreSQL: {e}")))?;
    if stmts.len() != 1 {
        return Err(Gap::new("more than one statement"));
    }
    let mut stmt = stmts.remove(0);
    if !matches!(stmt, Statement::Query(_)) {
        return Err(Gap::new("not a query"));
    }
    let mut w = Walker {
        resolve,
        refs: Vec::new(),
        gap: None,
        notes: Vec::new(),
    };
    let _ = stmt.visit(&mut w);
    if let Some(g) = w.gap {
        return Err(g);
    }
    let mut out = stmt.to_string();
    // Longest placeholder first, so ekos_ref_1 does not eat ekos_ref_10.
    for (i, model) in w.refs.iter().enumerate().rev() {
        out = out.replace(
            &format!("ekos_ref_{i}"),
            &format!("{{{{ ref('{model}') }}}}"),
        );
    }
    Ok(Rewritten {
        sql: out,
        notes: w.notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(n: &ObjectName) -> Result<String, Gap> {
        let t = n.0.last().unwrap().value.to_lowercase();
        if t == "secret" {
            Err(Gap::new("`secret` is not in the plan"))
        } else {
            Ok(format!("public__{t}"))
        }
    }

    #[test]
    fn rewrites_tables_casts_and_ilike() {
        let r = to_trino(
            "SELECT o.id, o.total::numeric AS t, orders.status FROM public.orders o JOIN customers ON customers.id = o.customer_id WHERE customers.name ILIKE 'a%' AND substring(o.code FROM 2 FOR 3) = 'x'",
            &resolve,
        )
        .unwrap();
        assert_eq!(
            r.sql,
            "SELECT o.id, CAST(o.total AS DECIMAL(38,10)) AS t, orders.status FROM {{ ref('public__orders') }} AS o JOIN {{ ref('public__customers') }} AS customers ON customers.id = o.customer_id WHERE lower(customers.name) LIKE lower('a%') AND SUBSTRING(o.code, 2, 3) = 'x'"
        );
        assert_eq!(r.notes.len(), 3, "{:?}", r.notes);
    }

    #[test]
    fn refuses_what_it_does_not_know() {
        for (sql, why) in [
            ("select distinct on (a) a from t", "DISTINCT ON"),
            ("select to_char(d, 'YYYY') from t", "to_char"),
            ("select a from t where a ~ 'x'", "operator"),
            ("select extract(dow from d) from t", "EXTRACT"),
            ("select a from secret", "not in the plan"),
            ("select * from t for update", "FOR UPDATE"),
            ("select x::interval from t", "cast to"),
        ] {
            let g = to_trino(sql, &resolve).unwrap_err();
            assert!(g.reason.contains(why), "{sql}: {g}");
        }
    }

    #[test]
    fn ten_or_more_tables_keep_their_refs_apart() {
        let sql = (0..11)
            .map(|i| format!("select a from t{i}"))
            .collect::<Vec<_>>()
            .join(" union all ");
        let r = to_trino(&sql, &resolve).unwrap();
        assert!(
            r.sql.contains("ref('public__t10')") && r.sql.contains("ref('public__t1')"),
            "{}",
            r.sql
        );
    }
}

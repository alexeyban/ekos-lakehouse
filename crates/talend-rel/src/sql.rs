//! SQL expressions — an ELT map's columns, filters and clauses, a statement's `SET` and `WHERE` —
//! as typed `etl-expr` trees (EKOS RFC 0267 phase 2 §SQL expressions).
//!
//! The text is parsed with sqlparser in the component's engine dialect and lowered by a whitelist:
//! every construct outside it is a gap with its text. Where the source engine and the target
//! differ, the rule is named in the result's notes and the result is `rewritten`.

use ekos_lakehouse_etl_expr::{BinOp, Expr, Func, Gap, KType, Lit, Lowered, UnOp};
use sqlparser::ast::{
    BinaryOperator, CastKind, DataType, DuplicateTreatment, Expr as SqlExpr, FunctionArg,
    FunctionArgExpr, FunctionArguments, GroupByExpr, ObjectName, OrderByExpr, SelectItem, SetExpr,
    Statement, TrimWhereField, UnaryOperator, Value,
};
use sqlparser::dialect::{
    Dialect, GenericDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect, SnowflakeDialect,
};

use crate::expr::ColumnLookup;

pub const NOTE_FOREIGN_HASH: &str = "Snowflake's `HASH` is its own 64-bit hash, which no other engine \
     computes: lowered to the MD5 of the same values as text. The keys are new values (the same rows \
     get the same key); do not compare them with the old table's keys";
pub const NOTE_TO_VARIANT: &str = "`TO_VARIANT` is lowered to a cast to text";
pub const NOTE_SQL_NOW: &str =
    "the current time is read once per statement; lowered to the run's start time";
pub const NOTE_COERCE: &str =
    "the source engine compares a number with text by converting it; the number is written as text";
pub const NOTE_DECIMAL_DIVISION: &str =
    "the source engine's `/` on integers keeps the fraction; the left operand is cast to decimal";
pub const NOTE_DECODE: &str = "`DECODE` matches null with null; lowered with IS NOT DISTINCT FROM";

/// The sqlparser dialect for an engine name (`snowflake`, `postgresql`, …); generic otherwise.
pub fn dialect(engine: Option<&str>) -> Box<dyn Dialect> {
    match engine.unwrap_or_default().to_ascii_lowercase().as_str() {
        "snowflake" => Box::new(SnowflakeDialect {}),
        "postgresql" | "postgres" | "greenplum" | "redshift" | "vertica" => {
            Box::new(PostgreSqlDialect {})
        }
        "mssql" | "sqlserver" | "sybase" => Box::new(MsSqlDialect {}),
        "mysql" | "mariadb" => Box::new(MySqlDialect {}),
        _ => Box::new(GenericDialect {}),
    }
}

/// Whether the engine's `/` on two integers truncates, as the target's does.
fn integer_division_truncates(engine: Option<&str>) -> bool {
    matches!(
        engine.unwrap_or_default().to_ascii_lowercase().as_str(),
        "postgresql" | "postgres" | "greenplum" | "redshift" | "mssql" | "sqlserver" | "sybase"
    )
}

/// Parses one SQL expression.
pub fn parse(src: &str, engine: Option<&str>) -> Result<SqlExpr, Gap> {
    let d = dialect(engine);
    sqlparser::parser::Parser::new(d.as_ref())
        .try_with_sql(src)
        .and_then(|mut p| {
            let e = p.parse_expr()?;
            p.expect_token(&sqlparser::tokenizer::Token::EOF)?;
            Ok(e)
        })
        .map_err(|e| Gap::new(format!("the SQL expression does not parse: {e}")))
}

/// What an ELT output's `OTHER_FILTER` text says.
#[derive(Debug, Default)]
pub struct Clauses {
    pub group_by: Vec<SqlExpr>,
    pub order_by: Vec<OrderByExpr>,
}

/// Parses `GROUP BY …` / `ORDER BY …` text by reading it after a placeholder `SELECT`.
pub fn parse_clauses(texts: &[String], engine: Option<&str>) -> Result<Clauses, Gap> {
    let mut out = Clauses::default();
    if texts.iter().all(|t| t.trim().is_empty()) {
        return Ok(out);
    }
    let sql = format!("SELECT 1 FROM t {}", texts.join(" "));
    let d = dialect(engine);
    let stmts = sqlparser::parser::Parser::parse_sql(d.as_ref(), &sql)
        .map_err(|e| Gap::new(format!("the clause does not parse: {e}")))?;
    let [Statement::Query(q)] = stmts.as_slice() else {
        return Err(Gap::new("the clause is not one GROUP BY / ORDER BY"));
    };
    let SetExpr::Select(s) = q.body.as_ref() else {
        return Err(Gap::new("the clause is not one GROUP BY / ORDER BY"));
    };
    if s.having.is_some() || s.selection.is_some() || q.limit.is_some() || s.qualify.is_some() {
        return Err(Gap::new(format!(
            "ELT clause `{}` (only GROUP BY and ORDER BY are lowered)",
            texts.join(" ")
        )));
    }
    if let GroupByExpr::Expressions(exprs, mods) = &s.group_by {
        if !mods.is_empty() {
            return Err(Gap::new("GROUP BY with a modifier (ROLLUP, CUBE, …)"));
        }
        out.group_by.clone_from(exprs);
    } else {
        return Err(Gap::new("GROUP BY ALL"));
    }
    if let Some(o) = &q.order_by {
        out.order_by.clone_from(&o.exprs);
    }
    debug_assert!(matches!(
        s.projection.as_slice(),
        [SelectItem::UnnamedExpr(_)]
    ));
    Ok(out)
}

/// An aggregate at the top of an expression: its function, its argument (`None` for `COUNT(*)`)
/// and whether it is `DISTINCT`.
pub fn aggregate_of(e: &SqlExpr) -> Option<(String, Option<&SqlExpr>, bool)> {
    let e = strip(e);
    let SqlExpr::Function(f) = e else {
        return None;
    };
    let name = last(&f.name).to_ascii_uppercase();
    if !matches!(name.as_str(), "SUM" | "COUNT" | "MIN" | "MAX" | "AVG") || f.over.is_some() {
        return None;
    }
    let FunctionArguments::List(l) = &f.args else {
        return None;
    };
    let distinct = matches!(l.duplicate_treatment, Some(DuplicateTreatment::Distinct));
    let arg = match l.args.as_slice() {
        [FunctionArg::Unnamed(FunctionArgExpr::Expr(a))] => Some(a),
        [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)] if name == "COUNT" => None,
        _ => return None,
    };
    Some((name.to_ascii_lowercase(), arg, distinct))
}

fn strip(e: &SqlExpr) -> &SqlExpr {
    match e {
        SqlExpr::Nested(inner) => strip(inner),
        other => other,
    }
}

fn last(n: &ObjectName) -> String {
    n.0.last().map(|i| i.value.clone()).unwrap_or_default()
}

/// Lowers one parsed SQL expression; aggregates are gaps here (see [`aggregate_of`]).
pub fn lower_ast(
    e: &SqlExpr,
    engine: Option<&str>,
    column: &ColumnLookup<'_>,
) -> Result<(Lowered, KType), Gap> {
    let mut l = SqlLowerer {
        engine,
        column,
        out: Lowered::exact(Expr::null()),
    };
    let (expr, ty) = l.lower(e)?;
    let mut out = l.out;
    out.expr = expr;
    Ok((out, ty))
}

/// Parses and lowers one SQL expression.
pub fn lower(
    src: &str,
    engine: Option<&str>,
    column: &ColumnLookup<'_>,
) -> Result<(Lowered, KType), Gap> {
    lower_ast(&parse(src, engine)?, engine, column)
}

struct SqlLowerer<'a, 'c> {
    engine: Option<&'a str>,
    column: &'a ColumnLookup<'c>,
    out: Lowered,
}

type Typed = (Expr, KType);

fn gap<T>(text: impl Into<String>) -> Result<T, Gap> {
    Err(Gap::new(text))
}

fn numeric(t: KType) -> bool {
    matches!(t, KType::Integer | KType::Number | KType::BigNumber)
}

fn wider(a: KType, b: KType) -> KType {
    use KType::*;
    match (a, b) {
        (BigNumber, _) | (_, BigNumber) => BigNumber,
        (Number, _) | (_, Number) => Number,
        (Integer, Integer) => Integer,
        (x, Unknown) | (Unknown, x) if numeric(x) => x,
        _ => Unknown,
    }
}

/// An Oracle / Snowflake date format of numeric fields as a Joda pattern.
fn joda(format: &str) -> Option<String> {
    const FIELDS: &[(&str, &str)] = &[
        ("YYYY", "yyyy"),
        ("HH24", "HH"),
        ("MM", "MM"),
        ("DD", "dd"),
        ("MI", "mm"),
        ("SS", "ss"),
        ("YY", "yy"),
    ];
    let mut out = String::new();
    let mut rest = format;
    while !rest.is_empty() {
        if let Some(c) = rest.chars().next().filter(|c| "-/:. ".contains(*c)) {
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let (tok, java) = FIELDS.iter().find(|(t, _)| {
            rest.len() >= t.len()
                && rest.is_char_boundary(t.len())
                && rest[..t.len()].eq_ignore_ascii_case(t)
        })?;
        out.push_str(java);
        rest = &rest[tok.len()..];
    }
    (!out.is_empty()).then_some(out)
}

fn sql_type(t: &DataType) -> Option<KType> {
    use DataType::*;
    Some(match t {
        Char(_) | Varchar(_) | String(_) | Text | CharVarying(_) | CharacterVarying(_)
        | Nvarchar(_) | Character(_) => KType::String,
        Int(_) | Integer(_) | BigInt(_) | SmallInt(_) | TinyInt(_) | Int64 => KType::Integer,
        Decimal(_) | Numeric(_) | Dec(_) | BigDecimal(_) | BigNumeric(_) => KType::BigNumber,
        Float(_) | Double | DoublePrecision | Real | Float64 | Float8 => KType::Number,
        Date => KType::Date,
        Timestamp(..) | Datetime(_) => KType::Timestamp,
        Boolean | Bool => KType::Boolean,
        Custom(name, _) => match last(name).to_ascii_uppercase().as_str() {
            "NUMBER" => KType::BigNumber,
            "VARCHAR2" | "NVARCHAR2" => KType::String,
            "TIMESTAMP_NTZ" | "TIMESTAMP_LTZ" | "TIMESTAMP_TZ" => KType::Timestamp,
            _ => return None,
        },
        _ => return None,
    })
}

impl SqlLowerer<'_, '_> {
    fn lower(&mut self, e: &SqlExpr) -> Result<Typed, Gap> {
        Ok(match e {
            SqlExpr::Nested(inner) => self.lower(inner)?,
            SqlExpr::Identifier(i) => self.col("", &i.value)?,
            SqlExpr::CompoundIdentifier(parts) => match parts.as_slice() {
                [q, c] => self.col(&q.value, &c.value)?,
                [.., q, c] => self.col(&q.value, &c.value)?,
                _ => return gap(format!("name `{e}`")),
            },
            SqlExpr::Value(v) => match v {
                Value::Number(n, _) => match n.parse::<i64>() {
                    Ok(i) => (Expr::int(i), KType::Integer),
                    Err(_) => (Expr::num(n), KType::Number),
                },
                Value::SingleQuotedString(s) => (Expr::str(s), KType::String),
                Value::Boolean(b) => (Expr::bool(*b), KType::Boolean),
                Value::Null => (Expr::null(), KType::Unknown),
                other => return gap(format!("literal `{other}`")),
            },
            SqlExpr::UnaryOp {
                op: UnaryOperator::Minus,
                expr,
            } if matches!(strip(expr), SqlExpr::Value(Value::Number(..))) => {
                let SqlExpr::Value(Value::Number(n, _)) = strip(expr) else {
                    unreachable!()
                };
                match n.parse::<i64>() {
                    Ok(i) => (Expr::int(-i), KType::Integer),
                    Err(_) => (Expr::num(&format!("-{n}")), KType::Number),
                }
            }
            SqlExpr::UnaryOp { op, expr } => {
                let (a, t) = self.lower(expr)?;
                match op {
                    UnaryOperator::Not => (Expr::negate(Expr::two_valued(a)), KType::Boolean),
                    UnaryOperator::Minus => (
                        Expr::Unary {
                            op: UnOp::Neg,
                            arg: Box::new(a),
                        },
                        t,
                    ),
                    UnaryOperator::Plus => (a, t),
                    other => return gap(format!("unary `{other}`")),
                }
            }
            SqlExpr::BinaryOp { left, op, right } => self.binary(left, op, right)?,
            SqlExpr::IsNull(a) => (Expr::is_null(self.lower(a)?.0), KType::Boolean),
            SqlExpr::IsNotNull(a) => (Expr::is_not_null(self.lower(a)?.0), KType::Boolean),
            SqlExpr::InList {
                expr,
                list,
                negated,
            } => {
                let (a, at) = self.lower(expr)?;
                let mut items = Vec::new();
                for i in list {
                    let (x, xt) = self.lower(i)?;
                    items.push(self.coerce(x, xt, at));
                }
                (
                    Expr::InList {
                        arg: Box::new(a),
                        list: items,
                        negated: *negated,
                    },
                    KType::Boolean,
                )
            }
            SqlExpr::Between {
                expr,
                negated,
                low,
                high,
            } => {
                let (a, _) = self.lower(expr)?;
                let (lo, _) = self.lower(low)?;
                let (hi, _) = self.lower(high)?;
                let inside = Expr::and(
                    Expr::bin(BinOp::GtEq, a.clone(), lo),
                    Expr::bin(BinOp::LtEq, a, hi),
                );
                (
                    if *negated {
                        Expr::negate(inside)
                    } else {
                        inside
                    },
                    KType::Boolean,
                )
            }
            SqlExpr::Like {
                negated,
                any: false,
                expr,
                pattern,
                escape_char: None,
            } => {
                let like = Expr::Like {
                    arg: Box::new(self.lower(expr)?.0),
                    pattern: Box::new(self.lower(pattern)?.0),
                };
                (
                    if *negated { Expr::negate(like) } else { like },
                    KType::Boolean,
                )
            }
            SqlExpr::Case {
                operand,
                conditions,
                results,
                else_result,
            } => {
                let subject = operand.as_ref().map(|o| self.lower(o)).transpose()?;
                let mut when = Vec::new();
                let mut ty = KType::Unknown;
                for (c, r) in conditions.iter().zip(results) {
                    let (c, ct) = self.lower(c)?;
                    let cond = match &subject {
                        Some((s, st)) => Expr::bin(BinOp::Eq, s.clone(), self.coerce(c, ct, *st)),
                        None => Expr::two_valued(c),
                    };
                    let (r, rt) = self.lower(r)?;
                    if ty == KType::Unknown {
                        ty = rt;
                    }
                    when.push((cond, r));
                }
                let otherwise = else_result.as_ref().map(|e| self.lower(e)).transpose()?;
                if ty == KType::Unknown
                    && let Some((_, t)) = &otherwise
                {
                    ty = *t;
                }
                (Expr::case(when, otherwise.map(|(e, _)| e)), ty)
            }
            SqlExpr::Cast {
                kind: CastKind::Cast | CastKind::DoubleColon,
                expr,
                data_type,
                format: None,
            } => {
                let (a, _) = self.lower(expr)?;
                let Some(to) = sql_type(data_type) else {
                    return gap(format!("cast to `{data_type}`"));
                };
                (Expr::cast(a, to), to)
            }
            SqlExpr::Trim {
                expr,
                trim_where,
                trim_what: None,
                trim_characters: None,
            } => {
                let (a, _) = self.lower(expr)?;
                let f = match trim_where {
                    None | Some(TrimWhereField::Both) => Func::Trim,
                    Some(TrimWhereField::Leading) => Func::LTrim,
                    Some(TrimWhereField::Trailing) => Func::RTrim,
                };
                (Expr::call(f, vec![a]), KType::String)
            }
            SqlExpr::Substring {
                expr,
                substring_from: Some(from),
                substring_for,
                ..
            } => {
                let mut args = vec![self.lower(expr)?.0, self.lower(from)?.0];
                if let Some(n) = substring_for {
                    args.push(self.lower(n)?.0);
                }
                (Expr::call(Func::Substr, args), KType::String)
            }
            SqlExpr::Function(f) => self.function(f)?,
            other => return gap(format!("SQL construct not translated: `{other}`")),
        })
    }

    fn col(&mut self, q: &str, c: &str) -> Result<Typed, Gap> {
        (self.column)(q, c).map_or_else(
            || {
                gap(if q.is_empty() {
                    format!("`{c}` is not an input column")
                } else {
                    format!("`{q}.{c}` is not an input column")
                })
            },
            Ok,
        )
    }

    /// A number compared with text: the number written as text, as the source engine compares.
    fn coerce(&mut self, e: Expr, from: KType, to: KType) -> Expr {
        match (&e, from, to) {
            (Expr::Lit { value }, f, KType::String) if numeric(f) => {
                self.out.note(NOTE_COERCE);
                let text = match value {
                    Lit::Int(i) => i.to_string(),
                    Lit::Num(n) => n.clone(),
                    _ => return e,
                };
                Expr::str(&text)
            }
            (Expr::Lit { .. }, KType::String, t) if numeric(t) => e,
            _ => e,
        }
    }

    fn binary(&mut self, l: &SqlExpr, op: &BinaryOperator, r: &SqlExpr) -> Result<Typed, Gap> {
        let (le, lt) = self.lower(l)?;
        let (re, rt) = self.lower(r)?;
        let b = KType::Boolean;
        let cmp = |s: &mut Self, o: BinOp| {
            let re = s.coerce(re.clone(), rt, lt);
            let le = s.coerce(le.clone(), lt, rt);
            Ok((Expr::bin(o, le, re), b))
        };
        match op {
            BinaryOperator::Plus => Ok((Expr::bin(BinOp::Add, le, re), wider(lt, rt))),
            BinaryOperator::Minus => Ok((Expr::bin(BinOp::Sub, le, re), wider(lt, rt))),
            BinaryOperator::Multiply => Ok((Expr::bin(BinOp::Mul, le, re), wider(lt, rt))),
            BinaryOperator::Modulo => Ok((Expr::bin(BinOp::Mod, le, re), wider(lt, rt))),
            BinaryOperator::Divide => {
                let t = wider(lt, rt);
                if t == KType::Integer && !integer_division_truncates(self.engine) {
                    self.out.note(NOTE_DECIMAL_DIVISION);
                    return Ok((
                        Expr::bin(BinOp::Div, Expr::cast(le, KType::BigNumber), re),
                        KType::BigNumber,
                    ));
                }
                Ok((Expr::bin(BinOp::Div, le, re), t))
            }
            BinaryOperator::StringConcat => {
                let s = |e: Expr, t: KType| {
                    if t == KType::String {
                        e
                    } else {
                        Expr::cast(e, KType::String)
                    }
                };
                Ok((
                    Expr::call(Func::Concat, vec![s(le, lt), s(re, rt)]),
                    KType::String,
                ))
            }
            BinaryOperator::Eq => cmp(self, BinOp::Eq),
            BinaryOperator::NotEq => cmp(self, BinOp::NotEq),
            BinaryOperator::Lt => cmp(self, BinOp::Lt),
            BinaryOperator::LtEq => cmp(self, BinOp::LtEq),
            BinaryOperator::Gt => cmp(self, BinOp::Gt),
            BinaryOperator::GtEq => cmp(self, BinOp::GtEq),
            BinaryOperator::And => Ok((Expr::and(le, re), b)),
            BinaryOperator::Or => Ok((Expr::or(le, re), b)),
            other => gap(format!("operator `{other}`")),
        }
    }

    fn args(&mut self, f: &sqlparser::ast::Function) -> Result<Vec<Typed>, Gap> {
        match &f.args {
            FunctionArguments::None => Ok(Vec::new()),
            FunctionArguments::List(l) => {
                if l.duplicate_treatment.is_some() || !l.clauses.is_empty() {
                    return gap(format!("function call `{f}`"));
                }
                l.args
                    .iter()
                    .map(|a| match a {
                        FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => self.lower(e),
                        _ => gap(format!("function argument `{a}`")),
                    })
                    .collect()
            }
            FunctionArguments::Subquery(_) => gap(format!("function over a subquery `{f}`")),
        }
    }

    fn function(&mut self, f: &sqlparser::ast::Function) -> Result<Typed, Gap> {
        let name = last(&f.name).to_ascii_uppercase();
        if f.over.is_some() || f.filter.is_some() || !f.within_group.is_empty() {
            return gap(format!("window or filtered function `{f}`"));
        }
        if matches!(name.as_str(), "SUM" | "COUNT" | "MIN" | "MAX" | "AVG") {
            return gap(format!("aggregate `{f}` outside a GROUP BY output"));
        }
        // The format of TO_DATE / TO_TIMESTAMP is read before its arguments are lowered.
        let format = match &f.args {
            FunctionArguments::List(l) => match l.args.get(1) {
                Some(FunctionArg::Unnamed(FunctionArgExpr::Expr(SqlExpr::Value(
                    Value::SingleQuotedString(s),
                )))) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        };
        let a = self.args(f)?;
        let n = a.len();
        let arg = |i: usize| a[i].0.clone();
        let s = KType::String;
        Ok(match (name.as_str(), n) {
            ("COALESCE" | "IFNULL" | "NVL", n) if n >= 2 => {
                let ty = a.iter().map(|(_, t)| *t).find(|t| *t != KType::Unknown);
                let ty = ty.unwrap_or(KType::Unknown);
                let items = a
                    .iter()
                    .map(|(e, t)| self.coerce(e.clone(), *t, ty))
                    .collect();
                (Expr::call(Func::Coalesce, items), ty)
            }
            ("NULLIF", 2) => (Expr::call(Func::NullIf, vec![arg(0), arg(1)]), a[0].1),
            ("IFF", 3) => {
                let ty = if a[1].1 == KType::Unknown {
                    a[2].1
                } else {
                    a[1].1
                };
                let then = self.coerce(arg(1), a[1].1, ty);
                let other = self.coerce(arg(2), a[2].1, ty);
                (
                    Expr::case(vec![(Expr::two_valued(arg(0)), then)], Some(other)),
                    ty,
                )
            }
            ("DECODE", n) if n >= 3 => {
                self.out.note(NOTE_DECODE);
                let subject = arg(0);
                let mut when = Vec::new();
                let mut i = 1;
                while i + 1 < n {
                    when.push((
                        Expr::bin(BinOp::NotDistinct, subject.clone(), arg(i)),
                        arg(i + 1),
                    ));
                    i += 2;
                }
                let otherwise = (i < n).then(|| arg(i));
                (Expr::case(when, otherwise), a[2].1)
            }
            ("TRIM", 1) => (Expr::call(Func::Trim, vec![arg(0)]), s),
            ("LTRIM", 1) => (Expr::call(Func::LTrim, vec![arg(0)]), s),
            ("RTRIM", 1) => (Expr::call(Func::RTrim, vec![arg(0)]), s),
            ("UPPER", 1) => (Expr::call(Func::Upper, vec![arg(0)]), s),
            ("LOWER", 1) => (Expr::call(Func::Lower, vec![arg(0)]), s),
            ("LENGTH" | "LEN", 1) => (Expr::call(Func::Length, vec![arg(0)]), KType::Integer),
            ("SUBSTR" | "SUBSTRING", 2 | 3) => (
                Expr::call(Func::Substr, a.iter().map(|(e, _)| e.clone()).collect()),
                s,
            ),
            ("REPLACE", 2 | 3) => (
                Expr::call(Func::Replace, a.iter().map(|(e, _)| e.clone()).collect()),
                s,
            ),
            ("CONCAT", n) if n >= 1 => (
                Expr::call(
                    Func::Concat,
                    a.iter()
                        .map(|(e, t)| {
                            if *t == KType::String {
                                e.clone()
                            } else {
                                Expr::cast(e.clone(), KType::String)
                            }
                        })
                        .collect(),
                ),
                s,
            ),
            ("LEAST" | "GREATEST", n) if n >= 2 => {
                let f = if name == "LEAST" {
                    Func::Least
                } else {
                    Func::Greatest
                };
                (
                    Expr::call(f, a.iter().map(|(e, _)| e.clone()).collect()),
                    a[0].1,
                )
            }
            ("LEFT", 2) => (
                Expr::call(Func::Substr, vec![arg(0), Expr::int(1), arg(1)]),
                s,
            ),
            ("RIGHT", 2) => (
                Expr::call(
                    Func::Substr,
                    vec![
                        arg(0),
                        Expr::Unary {
                            op: UnOp::Neg,
                            arg: Box::new(arg(1)),
                        },
                    ],
                ),
                s,
            ),
            ("TO_CHAR" | "TO_VARCHAR", 1) => (self.text(&a[0]), s),
            ("TO_NUMBER" | "TO_DECIMAL" | "TO_NUMERIC", 1) => {
                (Expr::cast(arg(0), KType::BigNumber), KType::BigNumber)
            }
            ("TO_NUMBER" | "TO_DECIMAL" | "TO_NUMERIC", 3)
                if matches!(
                    (&a[1].0, &a[2].0),
                    (
                        Expr::Lit { value: Lit::Int(_) },
                        Expr::Lit { value: Lit::Int(_) }
                    )
                ) =>
            {
                let (
                    Expr::Lit { value: Lit::Int(p) },
                    Expr::Lit {
                        value: Lit::Int(sc),
                    },
                ) = (&a[1].0, &a[2].0)
                else {
                    unreachable!()
                };
                (
                    Expr::Cast {
                        arg: Box::new(arg(0)),
                        to: KType::BigNumber,
                        length: i32::try_from(*p).unwrap_or(-1),
                        precision: i32::try_from(*sc).unwrap_or(-1),
                    },
                    KType::BigNumber,
                )
            }
            ("ABS", 1) => (Expr::call(Func::Abs, vec![arg(0)]), a[0].1),
            ("ROUND", 1 | 2) => (
                Expr::call(Func::Round, a.iter().map(|(e, _)| e.clone()).collect()),
                a[0].1,
            ),
            ("FLOOR", 1) => (Expr::call(Func::Floor, vec![arg(0)]), a[0].1),
            ("CEIL" | "CEILING", 1) => (Expr::call(Func::Ceil, vec![arg(0)]), a[0].1),
            ("CURRENT_TIMESTAMP" | "SYSDATE" | "SYSTIMESTAMP" | "GETDATE" | "NOW", 0) => {
                self.out.note(NOTE_SQL_NOW);
                (Expr::call(Func::Now, vec![]), KType::Timestamp)
            }
            ("CURRENT_DATE", 0) => {
                self.out.note(NOTE_SQL_NOW);
                (Expr::call(Func::Today, vec![]), KType::Date)
            }
            ("TO_DATE" | "TO_TIMESTAMP", 1) => {
                let to = if name == "TO_DATE" {
                    KType::Date
                } else {
                    KType::Timestamp
                };
                (Expr::cast(arg(0), to), to)
            }
            ("TO_DATE" | "TO_TIMESTAMP", 2) => {
                let Some(pattern) = format.as_deref().and_then(joda) else {
                    return gap(format!(
                        "{name} with a computed or non-numeric format `{}`",
                        format.unwrap_or_default()
                    ));
                };
                let parsed = Expr::call(Func::ParseDatetime, vec![arg(0), Expr::str(&pattern)]);
                if name == "TO_DATE" {
                    (Expr::cast(parsed, KType::Date), KType::Date)
                } else {
                    (parsed, KType::Timestamp)
                }
            }
            ("MD5", 1) => (Expr::call(Func::Md5Hex, vec![self.text(&a[0])]), s),
            ("HASH", n) if n >= 1 => {
                self.out.note(NOTE_FOREIGN_HASH);
                let mut parts = Vec::new();
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        parts.push(Expr::str("|"));
                    }
                    parts.push(self.text(x));
                }
                let joined = if parts.len() == 1 {
                    parts.remove(0)
                } else {
                    Expr::call(Func::Concat, parts)
                };
                (Expr::call(Func::Md5Hex, vec![joined]), s)
            }
            ("TO_VARIANT", 1) => {
                self.out.note(NOTE_TO_VARIANT);
                (self.text(&a[0]), s)
            }
            _ => {
                return gap(format!(
                    "SQL function `{name}` with {n} argument(s) is not in the catalogue"
                ));
            }
        })
    }

    fn text(&self, (e, t): &Typed) -> Expr {
        if *t == KType::String {
            e.clone()
        } else {
            Expr::cast(e.clone(), KType::String)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekos_lakehouse_etl_expr::Fidelity;

    fn cols(q: &str, c: &str) -> Option<(Expr, KType)> {
        match (q, c) {
            ("A", "QTY") => Some((Expr::col("A__QTY"), KType::Integer)),
            ("A", "N") => Some((Expr::col("A__N"), KType::Integer)),
            ("A", "CODE") => Some((Expr::col("A__CODE"), KType::String)),
            ("A", "D") => Some((Expr::col("A__D"), KType::String)),
            ("B", "PK") => Some((Expr::col("B__PK"), KType::String)),
            _ => None,
        }
    }

    fn sql(src: &str, engine: &str) -> (String, Vec<String>, Fidelity) {
        let (l, _) = lower(src, Some(engine), &cols).unwrap();
        (l.expr.sql(), l.notes, l.fidelity)
    }

    #[test]
    fn snowflake_functions_lower_with_their_rules_named() {
        let (s, notes, f) = sql(
            "IFF(TRIM(IFNULL(A.CODE, '')) = '', '0', TRIM(A.CODE))",
            "snowflake",
        );
        assert!(s.contains("CASE WHEN"), "{s}");
        assert!(s.contains("coalesce"), "{s}");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(f, Fidelity::Exact);
        let (s, notes, f) = sql("HASH(TO_VARIANT(A.CODE) || TO_VARIANT(A.QTY))", "snowflake");
        assert!(s.contains("md5"), "{s}");
        assert!(notes.iter().any(|n| n == NOTE_FOREIGN_HASH));
        assert_eq!(f, Fidelity::Rewritten);
        let (s, _, _) = sql("TO_DATE(A.D, 'YYYYMMDD')", "snowflake");
        assert!(s.contains("yyyyMMdd"), "{s}");
        assert!(lower("TO_DATE(A.D, 'Mon DD')", Some("snowflake"), &cols).is_err());
    }

    #[test]
    fn a_number_compared_with_text_is_written_as_text() {
        let (s, notes, _) = sql("COALESCE(TRIM(A.CODE), -1) <> '-1'", "snowflake");
        assert!(s.contains("'-1'") && !s.contains(", -1)"), "{s}");
        assert!(notes.iter().any(|n| n == NOTE_COERCE));
        let (s, _, _) = sql("A.CODE IN ('122', 123)", "snowflake");
        assert!(s.contains("'123'"), "{s}");
    }

    #[test]
    fn integer_division_keeps_the_fraction_where_the_engine_does() {
        let (s, notes, _) = sql("A.QTY / A.N", "snowflake");
        assert!(s.contains("CAST"), "{s}");
        assert!(notes.iter().any(|n| n == NOTE_DECIMAL_DIVISION));
        let (s, _, _) = sql("A.QTY / A.N", "postgresql");
        assert!(!s.contains("CAST"), "{s}");
    }

    #[test]
    fn aggregates_are_found_at_the_top_and_refused_elsewhere() {
        let e = parse("sum(A.QTY)", Some("snowflake")).unwrap();
        let (f, arg, distinct) = aggregate_of(&e).unwrap();
        assert_eq!(f, "sum");
        assert!(arg.is_some() && !distinct);
        let e = parse("COUNT(DISTINCT A.QTY)", Some("snowflake")).unwrap();
        assert!(aggregate_of(&e).unwrap().2);
        assert!(lower("1 + sum(A.QTY)", Some("snowflake"), &cols).is_err());
    }

    #[test]
    fn clauses_read_group_by_and_order_by_and_refuse_the_rest() {
        let c = parse_clauses(
            &["GROUP BY A.CODE, IFF(A.QTY > 0, 1, 0)".into()],
            Some("snowflake"),
        )
        .unwrap();
        assert_eq!(c.group_by.len(), 2);
        let c = parse_clauses(&["ORDER BY A.CODE DESC".into()], Some("snowflake")).unwrap();
        assert_eq!(c.order_by.len(), 1);
        assert!(
            parse_clauses(
                &["GROUP BY A.CODE HAVING count(*) > 1".into()],
                Some("snowflake")
            )
            .is_err()
        );
    }

    #[test]
    fn what_is_not_in_the_catalogue_is_a_gap_with_its_text() {
        let g = lower("SPLIT_PART(A.CODE, '-', 1)", Some("snowflake"), &cols).unwrap_err();
        assert!(g.reason.contains("SPLIT_PART"), "{}", g.reason);
        assert!(lower("A.MISSING", Some("snowflake"), &cols).is_err());
    }
}

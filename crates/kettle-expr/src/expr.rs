//! The typed expression tree and its Trino rendering.

use crate::types::KType;
use serde::{Deserialize, Serialize};

/// A literal value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "lit", content = "v", rename_all = "snake_case")]
pub enum Lit {
    Null,
    Str(String),
    Int(i64),
    /// A decimal number, kept as its text so nothing is lost to floating point.
    Num(String),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    /// `IS NOT DISTINCT FROM`: Kettle's `=`, where two nulls are equal.
    NotDistinct,
    /// `IS DISTINCT FROM`.
    Distinct,
    And,
    Or,
}

impl BinOp {
    fn sql(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Eq => "=",
            BinOp::NotEq => "<>",
            BinOp::Lt => "<",
            BinOp::LtEq => "<=",
            BinOp::Gt => ">",
            BinOp::GtEq => ">=",
            BinOp::NotDistinct => "IS NOT DISTINCT FROM",
            BinOp::Distinct => "IS DISTINCT FROM",
            BinOp::And => "AND",
            BinOp::Or => "OR",
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq
                | BinOp::NotEq
                | BinOp::Lt
                | BinOp::LtEq
                | BinOp::Gt
                | BinOp::GtEq
                | BinOp::NotDistinct
                | BinOp::Distinct
        )
    }
}

/// Date and time units for [`Func::DateAdd`] / [`Func::DateDiff`] / [`Func::DateTrunc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Millisecond,
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl Unit {
    fn sql(self) -> &'static str {
        match self {
            Unit::Millisecond => "millisecond",
            Unit::Second => "second",
            Unit::Minute => "minute",
            Unit::Hour => "hour",
            Unit::Day => "day",
            Unit::Week => "week",
            Unit::Month => "month",
            Unit::Quarter => "quarter",
            Unit::Year => "year",
        }
    }
}

/// The closed set of functions an expression may call. Every parser maps its source language
/// onto these; nothing else reaches SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Func {
    Upper,
    Lower,
    Trim,
    LTrim,
    RTrim,
    Length,
    /// `substr(s, start [, length])`, 1-based.
    Substr,
    /// Literal (not regex) replacement.
    Replace,
    /// Concatenation; the parser decides null handling.
    Concat,
    Coalesce,
    NullIf,
    Greatest,
    Least,
    Abs,
    /// Half away from zero (Trino's rule).
    Round,
    Ceil,
    Floor,
    Sqrt,
    Power,
    Mod,
    Sign,
    Ln,
    Log10,
    Exp,
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    DayOfYear,
    /// Sunday = 1 … Saturday = 7, as Java's `Calendar`.
    DayOfWeekSunday1,
    /// ISO 8601 week of year.
    IsoWeek,
    IsoYear,
    Quarter,
    DateTrunc(Unit),
    /// `date_add(unit, n, d)`: arguments `(d, n)`.
    DateAdd(Unit),
    /// `a - b` in whole units: arguments `(a, b)`.
    DateDiff(Unit),
    Now,
    Today,
    Lpad,
    Rpad,
    /// Position of `needle` in `s`, 1-based, 0 when absent: arguments `(s, needle)`.
    Strpos,
    StartsWith,
    /// Regex match anywhere (Trino `regexp_like`).
    RegexpLike,
    RegexpReplace,
    Md5Hex,
    Sha1Hex,
    Sha256Hex,
    Crc32,
    Levenshtein,
    Base64Encode,
    /// `format_datetime(d, javaPattern)`.
    FormatDatetime,
    /// `parse_datetime(s, javaPattern)`.
    ParseDatetime,
    /// Build a date from year, month, day.
    MakeDate,
    /// Each word's first letter upper-cased, the rest lower-cased.
    Initcap,
}

/// A typed expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "snake_case")]
pub enum Expr {
    Col {
        name: String,
    },
    Lit {
        value: Lit,
    },
    Unary {
        op: UnOp,
        arg: Box<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Call {
        func: Func,
        args: Vec<Expr>,
    },
    Case {
        when: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
    Cast {
        arg: Box<Expr>,
        to: KType,
        length: i32,
        precision: i32,
    },
    IsNull {
        arg: Box<Expr>,
        negated: bool,
    },
    InList {
        arg: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    Like {
        arg: Box<Expr>,
        pattern: Box<Expr>,
    },
}

impl Expr {
    pub fn col(name: &str) -> Expr {
        Expr::Col {
            name: name.to_string(),
        }
    }
    pub fn str(s: &str) -> Expr {
        Expr::Lit {
            value: Lit::Str(s.to_string()),
        }
    }
    pub fn int(i: i64) -> Expr {
        Expr::Lit { value: Lit::Int(i) }
    }
    pub fn num(s: &str) -> Expr {
        Expr::Lit {
            value: Lit::Num(s.to_string()),
        }
    }
    pub fn bool(b: bool) -> Expr {
        Expr::Lit {
            value: Lit::Bool(b),
        }
    }
    pub fn null() -> Expr {
        Expr::Lit { value: Lit::Null }
    }
    pub fn bin(op: BinOp, l: Expr, r: Expr) -> Expr {
        Expr::Binary {
            op,
            left: Box::new(l),
            right: Box::new(r),
        }
    }
    pub fn call(func: Func, args: Vec<Expr>) -> Expr {
        Expr::Call { func, args }
    }
    pub fn negate(e: Expr) -> Expr {
        Expr::Unary {
            op: UnOp::Not,
            arg: Box::new(e),
        }
    }
    pub fn is_null(e: Expr) -> Expr {
        Expr::IsNull {
            arg: Box::new(e),
            negated: false,
        }
    }
    pub fn is_not_null(e: Expr) -> Expr {
        Expr::IsNull {
            arg: Box::new(e),
            negated: true,
        }
    }
    pub fn and(l: Expr, r: Expr) -> Expr {
        Expr::bin(BinOp::And, l, r)
    }
    pub fn or(l: Expr, r: Expr) -> Expr {
        Expr::bin(BinOp::Or, l, r)
    }
    pub fn cast(e: Expr, to: KType) -> Expr {
        Expr::Cast {
            arg: Box::new(e),
            to,
            length: -1,
            precision: -1,
        }
    }
    pub fn case(when: Vec<(Expr, Expr)>, otherwise: Option<Expr>) -> Expr {
        Expr::Case {
            when,
            otherwise: otherwise.map(Box::new),
        }
    }
    /// `coalesce(e, false)`: a predicate that is never null, as Kettle's conditions are.
    pub fn two_valued(e: Expr) -> Expr {
        match e {
            Expr::IsNull { .. }
            | Expr::Lit {
                value: Lit::Bool(_),
            }
            | Expr::Binary {
                op: BinOp::NotDistinct | BinOp::Distinct,
                ..
            } => e,
            e => Expr::call(Func::Coalesce, vec![e, Expr::bool(false)]),
        }
    }

    /// Every column this expression reads, in order of first appearance.
    pub fn columns(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Col { name } = e
                && !out.contains(name)
            {
                out.push(name.clone());
            }
        });
        out
    }

    /// Calls `f` on this expression and every sub-expression, depth first.
    pub fn walk(&self, f: &mut dyn FnMut(&Expr)) {
        f(self);
        match self {
            Expr::Col { .. } | Expr::Lit { .. } => {}
            Expr::Unary { arg, .. } | Expr::Cast { arg, .. } | Expr::IsNull { arg, .. } => {
                arg.walk(f)
            }
            Expr::Binary { left, right, .. } => {
                left.walk(f);
                right.walk(f);
            }
            Expr::Call { args, .. } => args.iter().for_each(|a| a.walk(f)),
            Expr::Case { when, otherwise } => {
                for (c, v) in when {
                    c.walk(f);
                    v.walk(f);
                }
                if let Some(o) = otherwise {
                    o.walk(f);
                }
            }
            Expr::InList { arg, list, .. } => {
                arg.walk(f);
                list.iter().for_each(|a| a.walk(f));
            }
            Expr::Like { arg, pattern } => {
                arg.walk(f);
                pattern.walk(f);
            }
        }
    }

    /// Replaces every column reference through `f` (renames, or substitution of a computed
    /// field by its expression).
    pub fn map_columns(&self, f: &dyn Fn(&str) -> Expr) -> Expr {
        let m = |e: &Expr| Box::new(e.map_columns(f));
        match self {
            Expr::Col { name } => f(name),
            Expr::Lit { .. } => self.clone(),
            Expr::Unary { op, arg } => Expr::Unary {
                op: *op,
                arg: m(arg),
            },
            Expr::Binary { op, left, right } => Expr::Binary {
                op: *op,
                left: m(left),
                right: m(right),
            },
            Expr::Call { func, args } => Expr::Call {
                func: *func,
                args: args.iter().map(|a| a.map_columns(f)).collect(),
            },
            Expr::Case { when, otherwise } => Expr::Case {
                when: when
                    .iter()
                    .map(|(c, v)| (c.map_columns(f), v.map_columns(f)))
                    .collect(),
                otherwise: otherwise.as_ref().map(|o| m(o)),
            },
            Expr::Cast {
                arg,
                to,
                length,
                precision,
            } => Expr::Cast {
                arg: m(arg),
                to: *to,
                length: *length,
                precision: *precision,
            },
            Expr::IsNull { arg, negated } => Expr::IsNull {
                arg: m(arg),
                negated: *negated,
            },
            Expr::InList { arg, list, negated } => Expr::InList {
                arg: m(arg),
                list: list.iter().map(|a| a.map_columns(f)).collect(),
                negated: *negated,
            },
            Expr::Like { arg, pattern } => Expr::Like {
                arg: m(arg),
                pattern: m(pattern),
            },
        }
    }

    /// Trino SQL text.
    pub fn sql(&self) -> String {
        render(self)
    }
}

/// `"name"`, with embedded quotes doubled.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `'text'`, with embedded quotes doubled.
pub fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn render(e: &Expr) -> String {
    match e {
        Expr::Col { name } => quote_ident(name),
        Expr::Lit { value } => match value {
            Lit::Null => "NULL".into(),
            Lit::Str(s) => quote_str(s),
            Lit::Int(i) => i.to_string(),
            Lit::Num(n) => n.clone(),
            Lit::Bool(b) => {
                if *b {
                    "TRUE".into()
                } else {
                    "FALSE".into()
                }
            }
        },
        Expr::Unary { op: UnOp::Not, arg } => format!("NOT ({})", render(arg)),
        Expr::Unary { op: UnOp::Neg, arg } => format!("-({})", render(arg)),
        Expr::Binary { op, left, right } => {
            format!("({} {} {})", render(left), op.sql(), render(right))
        }
        Expr::Call { func, args } => render_call(*func, args),
        Expr::Case { when, otherwise } => {
            let mut s = String::from("CASE");
            for (c, v) in when {
                s.push_str(&format!(" WHEN {} THEN {}", render(c), render(v)));
            }
            if let Some(o) = otherwise {
                s.push_str(&format!(" ELSE {}", render(o)));
            }
            s.push_str(" END");
            s
        }
        Expr::Cast {
            arg,
            to,
            length,
            precision,
        } => {
            format!("CAST({} AS {})", render(arg), to.trino(*length, *precision))
        }
        Expr::IsNull { arg, negated } => {
            format!(
                "({} IS {}NULL)",
                render(arg),
                if *negated { "NOT " } else { "" }
            )
        }
        Expr::InList { arg, list, negated } => format!(
            "({} {}IN ({}))",
            render(arg),
            if *negated { "NOT " } else { "" },
            list.iter().map(render).collect::<Vec<_>>().join(", ")
        ),
        Expr::Like { arg, pattern } => format!("({} LIKE {})", render(arg), render(pattern)),
    }
}

fn render_call(func: Func, args: &[Expr]) -> String {
    let a: Vec<String> = args.iter().map(render).collect();
    let arg = |i: usize| a.get(i).cloned().unwrap_or_else(|| "NULL".into());
    let plain = |name: &str| format!("{name}({})", a.join(", "));
    match func {
        Func::Upper => plain("upper"),
        Func::Lower => plain("lower"),
        Func::Trim => plain("trim"),
        Func::LTrim => plain("ltrim"),
        Func::RTrim => plain("rtrim"),
        Func::Length => plain("length"),
        Func::Substr => plain("substr"),
        Func::Replace => plain("replace"),
        Func::Concat => plain("concat"),
        Func::Coalesce => plain("coalesce"),
        Func::NullIf => plain("nullif"),
        Func::Greatest => plain("greatest"),
        Func::Least => plain("least"),
        Func::Abs => plain("abs"),
        Func::Round => plain("round"),
        Func::Ceil => plain("ceiling"),
        Func::Floor => plain("floor"),
        Func::Sqrt => plain("sqrt"),
        Func::Power => plain("power"),
        Func::Mod => plain("mod"),
        Func::Sign => plain("sign"),
        Func::Ln => plain("ln"),
        Func::Log10 => plain("log10"),
        Func::Exp => plain("exp"),
        Func::Year => plain("year"),
        Func::Month => plain("month"),
        Func::Day => plain("day"),
        Func::Hour => plain("hour"),
        Func::Minute => plain("minute"),
        Func::Second => plain("second"),
        Func::DayOfYear => plain("day_of_year"),
        Func::DayOfWeekSunday1 => format!("(day_of_week({}) % 7 + 1)", arg(0)),
        Func::IsoWeek => plain("week_of_year"),
        Func::IsoYear => plain("year_of_week"),
        Func::Quarter => plain("quarter"),
        Func::DateTrunc(u) => format!("date_trunc('{}', {})", u.sql(), arg(0)),
        Func::DateAdd(u) => format!("date_add('{}', {}, {})", u.sql(), arg(1), arg(0)),
        Func::DateDiff(u) => format!("date_diff('{}', {}, {})", u.sql(), arg(1), arg(0)),
        Func::Now => "current_timestamp".into(),
        Func::Today => "current_date".into(),
        Func::Lpad => plain("lpad"),
        Func::Rpad => plain("rpad"),
        Func::Strpos => plain("strpos"),
        Func::StartsWith => plain("starts_with"),
        Func::RegexpLike => plain("regexp_like"),
        Func::RegexpReplace => plain("regexp_replace"),
        Func::Md5Hex => format!("lower(to_hex(md5(to_utf8({}))))", arg(0)),
        Func::Sha1Hex => format!("lower(to_hex(sha1(to_utf8({}))))", arg(0)),
        Func::Sha256Hex => format!("lower(to_hex(sha256(to_utf8({}))))", arg(0)),
        Func::Crc32 => format!("crc32(to_utf8({}))", arg(0)),
        Func::Levenshtein => plain("levenshtein_distance"),
        Func::Base64Encode => format!("to_base64(to_utf8({}))", arg(0)),
        Func::FormatDatetime => plain("format_datetime"),
        Func::ParseDatetime => plain("parse_datetime"),
        Func::MakeDate => format!(
            "date_add('day', {} - 1, date_add('month', {} - 1, date_add('year', {} - 1970, DATE '1970-01-01')))",
            arg(2),
            arg(1),
            arg(0)
        ),
        Func::Initcap => format!(
            "array_join(transform(split(lower({}), ' '), w -> concat(upper(substr(w, 1, 1)), substr(w, 2))), ' ')",
            arg(0)
        ),
    }
}

/// The type of `e` given the types of the columns it reads.
pub fn infer(e: &Expr, col_type: &dyn Fn(&str) -> KType) -> KType {
    match e {
        Expr::Col { name } => col_type(name),
        Expr::Lit { value } => match value {
            Lit::Null => KType::Unknown,
            Lit::Str(_) => KType::String,
            Lit::Int(_) => KType::Integer,
            Lit::Num(_) => KType::BigNumber,
            Lit::Bool(_) => KType::Boolean,
        },
        Expr::Unary { op: UnOp::Not, .. }
        | Expr::IsNull { .. }
        | Expr::InList { .. }
        | Expr::Like { .. } => KType::Boolean,
        Expr::Unary { op: UnOp::Neg, arg } => infer(arg, col_type),
        Expr::Binary { op, left, right } => match op {
            BinOp::And | BinOp::Or => KType::Boolean,
            o if o.is_comparison() => KType::Boolean,
            _ => {
                let (l, r) = (infer(left, col_type), infer(right, col_type));
                if l.is_temporal() {
                    l
                } else {
                    KType::widen(l, r)
                }
            }
        },
        Expr::Call { func, args } => {
            let first = || {
                args.first()
                    .map(|a| infer(a, col_type))
                    .unwrap_or(KType::Unknown)
            };
            match func {
                Func::Upper
                | Func::Lower
                | Func::Trim
                | Func::LTrim
                | Func::RTrim
                | Func::Substr
                | Func::Replace
                | Func::Concat
                | Func::Lpad
                | Func::Rpad
                | Func::RegexpReplace
                | Func::Md5Hex
                | Func::Sha1Hex
                | Func::Sha256Hex
                | Func::Base64Encode
                | Func::FormatDatetime
                | Func::Initcap => KType::String,
                Func::Length
                | Func::Year
                | Func::Month
                | Func::Day
                | Func::Hour
                | Func::Minute
                | Func::Second
                | Func::DayOfYear
                | Func::DayOfWeekSunday1
                | Func::IsoWeek
                | Func::IsoYear
                | Func::Quarter
                | Func::DateDiff(_)
                | Func::Strpos
                | Func::Crc32
                | Func::Levenshtein
                | Func::Sign => KType::Integer,
                Func::StartsWith | Func::RegexpLike => KType::Boolean,
                Func::Sqrt | Func::Power | Func::Ln | Func::Log10 | Func::Exp => KType::Number,
                Func::Now | Func::ParseDatetime => KType::Timestamp,
                Func::Today | Func::MakeDate => KType::Date,
                Func::DateTrunc(_) | Func::DateAdd(_) => first(),
                Func::Coalesce
                | Func::NullIf
                | Func::Greatest
                | Func::Least
                | Func::Abs
                | Func::Round
                | Func::Ceil
                | Func::Floor
                | Func::Mod => args
                    .iter()
                    .map(|a| infer(a, col_type))
                    .find(|t| *t != KType::Unknown)
                    .unwrap_or(KType::Unknown),
            }
        }
        Expr::Case { when, otherwise } => when
            .iter()
            .map(|(_, v)| v)
            .chain(otherwise.as_deref())
            .map(|v| infer(v, col_type))
            .find(|t| *t != KType::Unknown)
            .unwrap_or(KType::Unknown),
        Expr::Cast { to, .. } => *to,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_trino_sql() {
        let e = Expr::case(
            vec![(
                Expr::is_null(Expr::col("a\"b")),
                Expr::call(Func::DateAdd(Unit::Day), vec![Expr::col("d"), Expr::int(3)]),
            )],
            Some(Expr::call(Func::DayOfWeekSunday1, vec![Expr::col("d")])),
        );
        assert_eq!(
            e.sql(),
            "CASE WHEN (\"a\"\"b\" IS NULL) THEN date_add('day', 3, \"d\") ELSE (day_of_week(\"d\") % 7 + 1) END"
        );
        assert_eq!(Expr::str("it's").sql(), "'it''s'");
        assert_eq!(
            Expr::call(
                Func::DateDiff(Unit::Day),
                vec![Expr::col("a"), Expr::col("b")]
            )
            .sql(),
            "date_diff('day', \"b\", \"a\")"
        );
    }

    #[test]
    fn infers_types_and_lists_columns() {
        let e = Expr::bin(BinOp::Mul, Expr::col("qty"), Expr::col("price"));
        let t = |c: &str| {
            if c == "qty" {
                KType::Integer
            } else {
                KType::BigNumber
            }
        };
        assert_eq!(infer(&e, &t), KType::BigNumber);
        assert_eq!(e.columns(), vec!["qty".to_string(), "price".to_string()]);
        let renamed = e.map_columns(&|c| Expr::col(&format!("x_{c}")));
        assert_eq!(renamed.sql(), "(\"x_qty\" * \"x_price\")");
    }
}

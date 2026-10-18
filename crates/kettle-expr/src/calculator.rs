//! The Calculator step: each `calc_type` code as an expression over fields A, B and C.

use crate::condition::literal;
use crate::expr::{BinOp, Expr, Func, Unit, infer};
use crate::semantics::{Gap, Lowered, NOTE_CONCAT_NULLS, kettle_concat};
use crate::types::KType;

/// One `<calculation>` of a Calculator step, already read from XML.
#[derive(Debug, Clone, Default)]
pub struct Calculation {
    pub field_name: String,
    pub calc_type: String,
    pub field_a: Option<String>,
    pub field_b: Option<String>,
    pub field_c: Option<String>,
    pub value_type: KType,
    pub length: i32,
    pub precision: i32,
    pub conversion_mask: Option<String>,
}

const NOTE_ROUND: &str = "rounding of exact halves: Kettle's rule for this calc type may differ \
                          from Trino's round (half away from zero) on negative values";
const NOTE_INT_DIV: &str =
    "Integer ÷ Integer truncates, as Kettle's Calculator does for Integer fields";
const NOTE_DST: &str = "date differences are whole elapsed units; across a daylight-saving \
                        change Kettle (milliseconds ÷ unit) and Trino may differ by one";
const NOTE_WEEK: &str = "week of year: Kettle uses the JVM locale's calendar; written as ISO 8601 \
                         (weeks start Monday, week 1 holds the first Thursday)";

/// Lowers one calculation. `col_type` gives the type of fields A, B and C (and earlier
/// calculations of the same step, which the caller adds to the schema in order).
pub fn lower(c: &Calculation, col_type: &dyn Fn(&str) -> KType) -> Result<Lowered, Gap> {
    let field = |f: &Option<String>, letter: &str| -> Result<Expr, Gap> {
        f.as_deref()
            .filter(|s| !s.is_empty())
            .map(Expr::col)
            .ok_or_else(|| Gap::new(format!("{}: field {letter} is not set", c.calc_type)))
    };
    let a = || field(&c.field_a, "A");
    let b = || field(&c.field_b, "B");
    let cc = || field(&c.field_c, "C");
    let ty = |e: &Expr| infer(e, col_type);
    let ex = Lowered::exact;
    let rw = Lowered::rewritten;
    let call = Expr::call;
    let bin = Expr::bin;
    let rx = |e: Expr, pattern: &str| {
        call(
            Func::RegexpReplace,
            vec![e, Expr::str(pattern), Expr::str("")],
        )
    };
    let mut out = match c.calc_type.as_str() {
        "CONSTANT" => {
            let text = c.field_a.clone().unwrap_or_default();
            let t = if c.value_type == KType::Unknown {
                KType::String
            } else {
                c.value_type
            };
            ex(literal(&text, t, c.conversion_mask.as_deref())?)
        }
        "COPY_FIELD" | "COPY_OF_FIELD" => ex(a()?),
        "ADD" => {
            let (a, b) = (a()?, b()?);
            if ty(&a) == KType::String || ty(&b) == KType::String {
                rw(kettle_concat(vec![a, b]), NOTE_CONCAT_NULLS)
            } else if ty(&a).is_temporal() {
                return Err(Gap::new(
                    "ADD on a date: Kettle's meaning depends on the other field's type",
                ));
            } else {
                ex(bin(BinOp::Add, a, b))
            }
        }
        "ADD3" => ex(bin(BinOp::Add, bin(BinOp::Add, a()?, b()?), cc()?)),
        "SUBTRACT" => ex(bin(BinOp::Sub, a()?, b()?)),
        "MULTIPLY" => ex(bin(BinOp::Mul, a()?, b()?)),
        "DIVIDE" => {
            let (a, b) = (a()?, b()?);
            if ty(&a) == KType::Integer && ty(&b) == KType::Integer {
                rw(bin(BinOp::Div, a, b), NOTE_INT_DIV)
            } else {
                ex(bin(BinOp::Div, a, b))
            }
        }
        "SQUARE" => {
            let a = a()?;
            ex(bin(BinOp::Mul, a.clone(), a))
        }
        "SQUARE_ROOT" => ex(call(Func::Sqrt, vec![a()?])),
        "PERCENT_1" => ex(bin(
            BinOp::Div,
            bin(BinOp::Mul, Expr::num("100.0"), a()?),
            b()?,
        )),
        "PERCENT_2" => {
            let a = a()?;
            ex(bin(
                BinOp::Sub,
                a.clone(),
                bin(BinOp::Div, bin(BinOp::Mul, a, b()?), Expr::num("100.0")),
            ))
        }
        "PERCENT_3" => {
            let a = a()?;
            ex(bin(
                BinOp::Add,
                a.clone(),
                bin(BinOp::Div, bin(BinOp::Mul, a, b()?), Expr::num("100.0")),
            ))
        }
        "COMBINATION_1" => ex(bin(BinOp::Add, a()?, bin(BinOp::Mul, b()?, cc()?))),
        "COMBINATION_2" => {
            let (a, b) = (a()?, b()?);
            ex(call(
                Func::Sqrt,
                vec![bin(
                    BinOp::Add,
                    bin(BinOp::Mul, a.clone(), a),
                    bin(BinOp::Mul, b.clone(), b),
                )],
            ))
        }
        "ROUND_1" | "ROUND_STD_1" => rw(call(Func::Round, vec![a()?]), NOTE_ROUND),
        "ROUND_2" | "ROUND_STD_2" => rw(call(Func::Round, vec![a()?, b()?]), NOTE_ROUND),
        "NVL" => ex(call(Func::Coalesce, vec![a()?, b()?])),
        "ABS" => ex(call(Func::Abs, vec![a()?])),
        "CEIL" => ex(call(Func::Ceil, vec![a()?])),
        "FLOOR" => ex(call(Func::Floor, vec![a()?])),
        "REMAINDER" => ex(call(Func::Mod, vec![a()?, b()?])),
        "ADD_DAYS" => ex(call(Func::DateAdd(Unit::Day), vec![a()?, b()?])),
        "ADD_HOURS" => ex(call(Func::DateAdd(Unit::Hour), vec![a()?, b()?])),
        "ADD_MINUTES" => ex(call(Func::DateAdd(Unit::Minute), vec![a()?, b()?])),
        "ADD_SECONDS" => ex(call(Func::DateAdd(Unit::Second), vec![a()?, b()?])),
        "ADD_MONTHS" => ex(call(Func::DateAdd(Unit::Month), vec![a()?, b()?])),
        "YEAR_OF_DATE" => ex(call(Func::Year, vec![a()?])),
        "MONTH_OF_DATE" => ex(call(Func::Month, vec![a()?])),
        "DAY_OF_YEAR" => ex(call(Func::DayOfYear, vec![a()?])),
        "DAY_OF_MONTH" => ex(call(Func::Day, vec![a()?])),
        "DAY_OF_WEEK" => ex(call(Func::DayOfWeekSunday1, vec![a()?])),
        "WEEK_OF_YEAR" => rw(call(Func::IsoWeek, vec![a()?]), NOTE_WEEK),
        "WEEK_OF_YEAR_ISO8601" => ex(call(Func::IsoWeek, vec![a()?])),
        "YEAR_OF_DATE_ISO8601" => ex(call(Func::IsoYear, vec![a()?])),
        "QUARTER_OF_DATE" => ex(call(Func::Quarter, vec![a()?])),
        "HOUR_OF_DAY" => ex(call(Func::Hour, vec![a()?])),
        "MINUTE_OF_HOUR" => ex(call(Func::Minute, vec![a()?])),
        "SECOND_OF_MINUTE" => ex(call(Func::Second, vec![a()?])),
        "REMOVE_TIME_FROM_DATE" => ex(call(Func::DateTrunc(Unit::Day), vec![a()?])),
        "DATE_DIFF" => rw(call(Func::DateDiff(Unit::Day), vec![a()?, b()?]), NOTE_DST),
        "DATE_DIFF_MSEC" => ex(call(Func::DateDiff(Unit::Millisecond), vec![a()?, b()?])),
        "DATE_DIFF_SEC" => ex(call(Func::DateDiff(Unit::Second), vec![a()?, b()?])),
        "DATE_DIFF_MN" => ex(call(Func::DateDiff(Unit::Minute), vec![a()?, b()?])),
        "DATE_DIFF_HR" => rw(call(Func::DateDiff(Unit::Hour), vec![a()?, b()?]), NOTE_DST),
        "UPPER_CASE" => ex(call(Func::Upper, vec![a()?])),
        "LOWER_CASE" => ex(call(Func::Lower, vec![a()?])),
        "INITCAP" => rw(
            call(Func::Initcap, vec![a()?]),
            "initcap: words split on single spaces only (Kettle also splits on other whitespace)",
        ),
        "STRING_LEN" => ex(call(Func::Length, vec![a()?])),
        "REMOVE_CR" => ex(rx(a()?, "\\r")),
        "REMOVE_LF" => ex(rx(a()?, "\\n")),
        "REMOVE_CRLF" => ex(rx(a()?, "[\\r\\n]")),
        "REMOVE_TAB" => ex(rx(a()?, "\\t")),
        "GET_ONLY_DIGITS" => ex(rx(a()?, "[^0-9]")),
        "REMOVE_DIGITS" => ex(rx(a()?, "[0-9]")),
        "MD5" => ex(call(Func::Md5Hex, vec![a()?])),
        "SHA1" => ex(call(Func::Sha1Hex, vec![a()?])),
        "CRC32" => ex(call(Func::Crc32, vec![a()?])),
        "LEVENSHTEIN_DISTANCE" => ex(call(Func::Levenshtein, vec![a()?, b()?])),
        "BASE64_ENCODE" | "BASE64_ENCODE_PADDED" => ex(call(Func::Base64Encode, vec![a()?])),
        other => {
            return Err(Gap::new(format!(
                "Calculator `{other}` has no SQL equivalent in the translator"
            )));
        }
    };
    // The step declares the result type; make it explicit when it differs from the
    // expression's own type.
    if c.value_type != KType::Unknown && infer(&out.expr, col_type) != c.value_type {
        out.expr = Expr::Cast {
            arg: Box::new(out.expr),
            to: c.value_type,
            length: c.length,
            precision: c.precision,
        };
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantics::Fidelity;

    fn calc(t: &str, a: &str, b: &str, vt: KType) -> Calculation {
        Calculation {
            field_name: "out".into(),
            calc_type: t.into(),
            field_a: Some(a.into()),
            field_b: (!b.is_empty()).then(|| b.into()),
            value_type: vt,
            length: -1,
            precision: -1,
            ..Default::default()
        }
    }

    fn types(c: &str) -> KType {
        match c {
            "qty" | "n" => KType::Integer,
            "price" => KType::BigNumber,
            "d" => KType::Date,
            _ => KType::String,
        }
    }

    #[test]
    fn arithmetic_dates_and_strings() {
        let l = lower(&calc("MULTIPLY", "qty", "price", KType::BigNumber), &types).unwrap();
        assert_eq!(
            (l.expr.sql().as_str(), l.fidelity),
            ("(\"qty\" * \"price\")", Fidelity::Exact)
        );
        let l = lower(&calc("ADD", "first", "last", KType::String), &types).unwrap();
        assert_eq!(l.fidelity, Fidelity::Rewritten);
        let l = lower(&calc("DAY_OF_WEEK", "d", "", KType::Integer), &types).unwrap();
        assert_eq!(l.expr.sql(), "(day_of_week(\"d\") % 7 + 1)");
        let l = lower(&calc("ADD_DAYS", "d", "n", KType::Date), &types).unwrap();
        assert_eq!(l.expr.sql(), "date_add('day', \"n\", \"d\")");
        let l = lower(&calc("DIVIDE", "qty", "n", KType::Integer), &types).unwrap();
        assert_eq!(l.fidelity, Fidelity::Rewritten);
    }

    #[test]
    fn constants_casts_and_gaps() {
        let mut c = calc("CONSTANT", "42", "", KType::Integer);
        assert_eq!(lower(&c, &types).unwrap().expr.sql(), "42");
        c.value_type = KType::Number;
        c.field_a = Some("1.5".into());
        assert_eq!(lower(&c, &types).unwrap().expr.sql(), "CAST(1.5 AS double)");
        assert!(lower(&calc("METAPHONE", "x", "", KType::String), &types).is_err());
        assert!(lower(&calc("ADD", "", "x", KType::String), &types).is_err());
    }
}

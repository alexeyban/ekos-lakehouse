//! The Formula step's OpenFormula (Pentaho LibFormula) expressions: `[field]` references,
//! `"text"`, `;`-separated arguments, `&` concatenation, `=`/`<>` comparisons.

use crate::expr::{BinOp, Expr, Func, UnOp, infer};
use crate::semantics::{Gap, Lowered, NOTE_CONCAT_NULLS, kettle_concat};
use crate::types::KType;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(String),
    Str(String),
    Field(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Sep,
}

fn lex(src: &str) -> Result<Vec<Tok>, Gap> {
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        match c {
            ' ' | '\t' | '\r' | '\n' => i += 1,
            '[' => {
                let end = cs[i..]
                    .iter()
                    .position(|&x| x == ']')
                    .ok_or_else(|| Gap::new("unclosed `[` field reference"))?;
                let name: String = cs[i + 1..i + end].iter().collect();
                // `[.field]` is LibFormula's relative form.
                out.push(Tok::Field(name.trim_start_matches('.').to_string()));
                i += end + 1;
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match cs.get(i) {
                        None => return Err(Gap::new("unclosed string literal")),
                        Some('"') if cs.get(i + 1) == Some(&'"') => {
                            s.push('"');
                            i += 2;
                        }
                        Some('"') => {
                            i += 1;
                            break;
                        }
                        Some(ch) => {
                            s.push(*ch);
                            i += 1;
                        }
                    }
                }
                out.push(Tok::Str(s));
            }
            '0'..='9' | '.' => {
                let start = i;
                while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                    i += 1;
                }
                out.push(Tok::Num(cs[start..i].iter().collect()));
            }
            'A'..='Z' | 'a'..='z' | '_' => {
                let start = i;
                while i < cs.len() && (cs[i].is_ascii_alphanumeric() || matches!(cs[i], '_' | '.'))
                {
                    i += 1;
                }
                out.push(Tok::Ident(
                    cs[start..i].iter().collect::<String>().to_ascii_uppercase(),
                ));
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ';' | ',' => {
                out.push(Tok::Sep);
                i += 1;
            }
            '<' if cs.get(i + 1) == Some(&'>') => {
                out.push(Tok::Op("<>"));
                i += 2;
            }
            '<' if cs.get(i + 1) == Some(&'=') => {
                out.push(Tok::Op("<="));
                i += 2;
            }
            '>' if cs.get(i + 1) == Some(&'=') => {
                out.push(Tok::Op(">="));
                i += 2;
            }
            '+' | '-' | '*' | '/' | '^' | '&' | '=' | '<' | '>' | '%' => {
                let op = match c {
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    '/' => "/",
                    '^' => "^",
                    '&' => "&",
                    '=' => "=",
                    '<' => "<",
                    '>' => ">",
                    _ => "%",
                };
                out.push(Tok::Op(op));
                i += 1;
            }
            other => {
                return Err(Gap::new(format!(
                    "unexpected character `{other}` in formula"
                )));
            }
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    col_type: &'a dyn Fn(&str) -> KType,
    out: Lowered,
}

/// Lowers a formula.
pub fn lower(src: &str, col_type: &dyn Fn(&str) -> KType) -> Result<Lowered, Gap> {
    let src = src.trim().trim_start_matches('=');
    let mut p = Parser {
        toks: lex(src)?,
        pos: 0,
        col_type,
        out: Lowered::exact(Expr::null()),
    };
    let e = p.expr(0)?;
    if p.pos != p.toks.len() {
        return Err(Gap::new(format!(
            "unexpected `{:?}` after the formula",
            p.toks[p.pos]
        )));
    }
    let mut out = p.out;
    out.expr = e;
    Ok(out)
}

fn binding(op: &str) -> Option<(u8, u8)> {
    Some(match op {
        "=" | "<>" | "<" | "<=" | ">" | ">=" => (1, 2),
        "&" => (3, 4),
        "+" | "-" => (5, 6),
        "*" | "/" => (7, 8),
        "^" => (10, 9),
        _ => return None,
    })
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn ty(&self, e: &Expr) -> KType {
        infer(e, self.col_type)
    }

    fn expr(&mut self, min: u8) -> Result<Expr, Gap> {
        let mut lhs = self.prefix()?;
        loop {
            if let Some(Tok::Op("%")) = self.peek() {
                self.pos += 1;
                lhs = Expr::bin(BinOp::Div, lhs, Expr::num("100.0"));
                continue;
            }
            let Some(Tok::Op(op)) = self.peek().cloned() else {
                break;
            };
            let Some((l, r)) = binding(op) else { break };
            if l < min {
                break;
            }
            self.pos += 1;
            let rhs = self.expr(r)?;
            lhs = self.binary(op, lhs, rhs);
        }
        Ok(lhs)
    }

    fn binary(&mut self, op: &str, l: Expr, r: Expr) -> Expr {
        let text = self.ty(&l) == KType::String || self.ty(&r) == KType::String;
        let cmp = |op| -> BinOp {
            match op {
                "=" => BinOp::Eq,
                "<>" => BinOp::NotEq,
                "<" => BinOp::Lt,
                "<=" => BinOp::LtEq,
                ">" => BinOp::Gt,
                _ => BinOp::GtEq,
            }
        };
        match op {
            "&" => {
                self.out.note(NOTE_CONCAT_NULLS);
                kettle_concat(vec![l, r])
            }
            "+" => Expr::bin(BinOp::Add, l, r),
            "-" => Expr::bin(BinOp::Sub, l, r),
            "*" => Expr::bin(BinOp::Mul, l, r),
            "/" => {
                // OpenFormula divides exactly; Trino truncates Integer ÷ Integer.
                if self.ty(&l) == KType::Integer && self.ty(&r) == KType::Integer {
                    Expr::bin(BinOp::Div, Expr::cast(l, KType::Number), r)
                } else {
                    Expr::bin(BinOp::Div, l, r)
                }
            }
            "^" => Expr::call(Func::Power, vec![l, r]),
            o if text => {
                self.out
                    .note("OpenFormula compares text case-insensitively: both sides lower-cased");
                Expr::bin(
                    cmp(o),
                    Expr::call(Func::Lower, vec![l]),
                    Expr::call(Func::Lower, vec![r]),
                )
            }
            o => Expr::bin(cmp(o), l, r),
        }
    }

    fn prefix(&mut self) -> Result<Expr, Gap> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(match n.parse::<i64>() {
                Ok(i) if !n.contains('.') => Expr::int(i),
                _ => Expr::num(&n),
            }),
            Some(Tok::Str(s)) => Ok(if s.is_empty() {
                Expr::null()
            } else {
                Expr::str(&s)
            }),
            Some(Tok::Field(f)) => Ok(Expr::col(&f)),
            Some(Tok::Op("-")) => {
                let e = self.expr(9)?;
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    arg: Box::new(e),
                })
            }
            Some(Tok::Op("+")) => self.expr(9),
            Some(Tok::LParen) => {
                let e = self.expr(0)?;
                match self.next() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err(Gap::new("missing `)`")),
                }
            }
            Some(Tok::Ident(name)) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() == Some(&Tok::RParen) {
                        self.pos += 1;
                    } else {
                        loop {
                            args.push(self.expr(0)?);
                            match self.next() {
                                Some(Tok::Sep) => continue,
                                Some(Tok::RParen) => break,
                                _ => return Err(Gap::new(format!("bad argument list of {name}"))),
                            }
                        }
                    }
                    self.function(&name, args)
                } else {
                    match name.as_str() {
                        "TRUE" => Ok(Expr::bool(true)),
                        "FALSE" => Ok(Expr::bool(false)),
                        other => Err(Gap::new(format!(
                            "unknown name `{other}` (fields are written [name])"
                        ))),
                    }
                }
            }
            other => Err(Gap::new(format!("unexpected `{other:?}` in formula"))),
        }
    }

    fn function(&mut self, name: &str, args: Vec<Expr>) -> Result<Expr, Gap> {
        let n = args.len();
        let arity = |lo: usize, hi: usize| -> Result<(), Gap> {
            if n < lo || n > hi {
                Err(Gap::new(format!(
                    "{name} takes {lo}–{hi} arguments, has {n}"
                )))
            } else {
                Ok(())
            }
        };
        let a = |i: usize| args[i].clone();
        let call = Expr::call;
        Ok(match name {
            "IF" => {
                arity(2, 3)?;
                let otherwise = if n == 3 { a(2) } else { Expr::bool(false) };
                Expr::case(vec![(Expr::two_valued(a(0)), a(1))], Some(otherwise))
            }
            "AND" | "OR" => {
                arity(1, 255)?;
                let op = if name == "AND" { BinOp::And } else { BinOp::Or };
                args.into_iter()
                    .reduce(|l, r| Expr::bin(op, l, r))
                    .expect("arity")
            }
            "NOT" => {
                arity(1, 1)?;
                Expr::negate(a(0))
            }
            "TRUE" => Expr::bool(true),
            "FALSE" => Expr::bool(false),
            "NA" => Expr::null(),
            "ISBLANK" | "ISNA" | "ISNULL" => {
                arity(1, 1)?;
                Expr::is_null(a(0))
            }
            "LEN" => {
                arity(1, 1)?;
                call(Func::Length, vec![a(0)])
            }
            "LEFT" => {
                arity(1, 2)?;
                let len = if n == 2 { a(1) } else { Expr::int(1) };
                call(Func::Substr, vec![a(0), Expr::int(1), len])
            }
            "RIGHT" => {
                arity(1, 2)?;
                let len = if n == 2 { a(1) } else { Expr::int(1) };
                let start = call(
                    Func::Greatest,
                    vec![
                        Expr::bin(
                            BinOp::Add,
                            Expr::bin(BinOp::Sub, call(Func::Length, vec![a(0)]), len),
                            Expr::int(1),
                        ),
                        Expr::int(1),
                    ],
                );
                call(Func::Substr, vec![a(0), start])
            }
            "MID" => {
                arity(3, 3)?;
                call(Func::Substr, vec![a(0), a(1), a(2)])
            }
            "UPPER" => call(Func::Upper, vec![a(0)]),
            "LOWER" => call(Func::Lower, vec![a(0)]),
            "TRIM" => {
                arity(1, 1)?;
                // OpenFormula TRIM also collapses inner runs of spaces.
                call(
                    Func::Trim,
                    vec![call(
                        Func::RegexpReplace,
                        vec![a(0), Expr::str(" +"), Expr::str(" ")],
                    )],
                )
            }
            "PROPER" => {
                self.out.note("PROPER: words split on single spaces only");
                call(Func::Initcap, vec![a(0)])
            }
            "CONCATENATE" => {
                self.out.note(NOTE_CONCAT_NULLS);
                kettle_concat(args)
            }
            "SUBSTITUTE" => {
                arity(3, 3)?;
                call(Func::Replace, vec![a(0), a(1), a(2)])
            }
            "FIND" => {
                arity(2, 2)?;
                self.out
                    .note("FIND of a missing text is an error in OpenFormula; here it is 0");
                call(Func::Strpos, vec![a(1), a(0)])
            }
            "EXACT" => {
                arity(2, 2)?;
                Expr::bin(BinOp::Eq, a(0), a(1))
            }
            "ROUND" => {
                arity(1, 2)?;
                call(Func::Round, args)
            }
            "INT" => call(Func::Floor, vec![a(0)]),
            "ABS" => call(Func::Abs, vec![a(0)]),
            "MOD" => {
                arity(2, 2)?;
                // OpenFormula: the result takes the divisor's sign.
                Expr::bin(
                    BinOp::Sub,
                    a(0),
                    Expr::bin(
                        BinOp::Mul,
                        a(1),
                        call(
                            Func::Floor,
                            vec![Expr::bin(BinOp::Div, Expr::cast(a(0), KType::Number), a(1))],
                        ),
                    ),
                )
            }
            "POWER" => call(Func::Power, args),
            "SQRT" => call(Func::Sqrt, args),
            "LN" => call(Func::Ln, args),
            "LOG10" => call(Func::Log10, args),
            "EXP" => call(Func::Exp, args),
            "SIGN" => call(Func::Sign, args),
            "MIN" | "MAX" => {
                arity(1, 255)?;
                self.out.note(format!(
                    "{name}: a null argument makes the result null (OpenFormula skips blanks)"
                ));
                call(
                    if name == "MIN" {
                        Func::Least
                    } else {
                        Func::Greatest
                    },
                    args,
                )
            }
            "SUM" => {
                arity(1, 255)?;
                args.into_iter()
                    .reduce(|l, r| Expr::bin(BinOp::Add, l, r))
                    .expect("arity")
            }
            "NOW" => call(Func::Now, vec![]),
            "TODAY" => call(Func::Today, vec![]),
            "DATE" => {
                arity(3, 3)?;
                call(Func::MakeDate, args)
            }
            "YEAR" => call(Func::Year, vec![a(0)]),
            "MONTH" => call(Func::Month, vec![a(0)]),
            "DAY" => call(Func::Day, vec![a(0)]),
            "HOUR" => call(Func::Hour, vec![a(0)]),
            "MINUTE" => call(Func::Minute, vec![a(0)]),
            "SECOND" => call(Func::Second, vec![a(0)]),
            "WEEKDAY" => {
                arity(1, 2)?;
                let kind = match args.get(1) {
                    None => 1,
                    Some(Expr::Lit {
                        value: crate::expr::Lit::Int(k),
                    }) => *k,
                    Some(_) => return Err(Gap::new("WEEKDAY with a computed type")),
                };
                match kind {
                    1 => call(Func::DayOfWeekSunday1, vec![a(0)]),
                    2 => Expr::iso_dow(a(0)),
                    3 => Expr::bin(BinOp::Sub, Expr::iso_dow(a(0)), Expr::int(1)),
                    k => return Err(Gap::new(format!("WEEKDAY type {k}"))),
                }
            }
            "DAYS" => {
                arity(2, 2)?;
                call(Func::DateDiff(crate::expr::Unit::Day), vec![a(0), a(1)])
            }
            "VALUE" => Expr::cast(a(0), KType::Number),
            "CHOOSE" => {
                arity(2, 255)?;
                let when = args[1..]
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        (
                            Expr::bin(BinOp::Eq, a(0), Expr::int(i as i64 + 1)),
                            v.clone(),
                        )
                    })
                    .collect();
                Expr::case(when, None)
            }
            other => {
                return Err(Gap::new(format!(
                    "formula function {other} is not supported"
                )));
            }
        })
    }
}

impl Expr {
    /// ISO day of week (Monday = 1).
    fn iso_dow(d: Expr) -> Expr {
        // ((sunday1 + 5) % 7) + 1
        Expr::bin(
            BinOp::Add,
            Expr::bin(
                BinOp::Mod,
                Expr::bin(
                    BinOp::Add,
                    Expr::call(Func::DayOfWeekSunday1, vec![d]),
                    Expr::int(5),
                ),
                Expr::int(7),
            ),
            Expr::int(1),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantics::Fidelity;

    fn types(c: &str) -> KType {
        match c {
            "qty" | "y" | "m" | "d" => KType::Integer,
            "price" | "UnitPrice" | "StandardCost" | "OrderQty" => KType::BigNumber,
            "dt" => KType::Date,
            _ => KType::String,
        }
    }

    #[test]
    fn the_corpus_formulas() {
        let l = lower("[y]*10000 + [m]*100 + [d]", &types).unwrap();
        assert_eq!(l.expr.sql(), "(((\"y\" * 10000) + (\"m\" * 100)) + \"d\")");
        assert_eq!(l.fidelity, Fidelity::Exact);
        let l = lower("([UnitPrice] - [StandardCost]) * [OrderQty]", &types).unwrap();
        assert_eq!(
            l.expr.sql(),
            "((\"UnitPrice\" - \"StandardCost\") * \"OrderQty\")"
        );
    }

    #[test]
    fn text_division_if_and_functions() {
        let l = lower("IF([name] = \"x\"; LEFT([name]; 2); \"other\")", &types).unwrap();
        assert_eq!(
            l.expr.sql(),
            "CASE WHEN coalesce((lower(\"name\") = lower('x')), FALSE) THEN substr(\"name\", 1, 2) ELSE 'other' END"
        );
        assert_eq!(l.fidelity, Fidelity::Rewritten);
        assert_eq!(
            lower("[qty] / [d]", &types).unwrap().expr.sql(),
            "(CAST(\"qty\" AS double) / \"d\")"
        );
        assert_eq!(lower("2^3", &types).unwrap().expr.sql(), "power(2, 3)");
        assert_eq!(lower("50%", &types).unwrap().expr.sql(), "(50 / 100.0)");
        assert_eq!(
            lower("WEEKDAY([dt]; 2)", &types).unwrap().expr.sql(),
            "((((day_of_week(\"dt\") % 7 + 1) + 5) % 7) + 1)"
        );
        assert!(lower("TEXT([dt]; \"yyyy\")", &types).is_err());
        assert!(lower("[a] +", &types).is_err());
    }
}

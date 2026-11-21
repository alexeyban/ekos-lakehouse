//! Java lowering with Kettle's rules (RFC 0202). The Java lexer and parser are engine-neutral
//! and live in `ekos-lakehouse-etl-expr` (RFC 0266); they are re-exported here.

pub use ekos_lakehouse_etl_expr::java::*;

use crate::expr::{BinOp, Expr, Func, UnOp};
use crate::semantics::{Gap, Lowered, NOTE_REGEX_FULL, full_match};
use crate::types::KType;

pub const NOTE_NPE: &str = "a null value here would stop the Kettle step with a \
                            NullPointerException; the SQL yields null instead";
pub const NOTE_JAVA_CONCAT: &str =
    "Java string concatenation prints a null operand as \"null\"; kept as written";

/// Lowers Java expressions over fields (and, for UDJC, local variables) to SQL.
pub struct Lowerer<'a> {
    /// The Java type of a name, `None` when the name is not known.
    pub name_type: &'a dyn Fn(&str) -> Option<JType>,
    /// The SQL a name stands for (a field reads its column; a local, its assigned expression).
    pub name_expr: &'a dyn Fn(&str) -> Expr,
    pub out: Lowered,
    /// JavaScript (Rhino) semantics instead of Java's (RFC 0203).
    pub js: bool,
}

impl<'a> Lowerer<'a> {
    /// A lowerer over Kettle fields only (Java expression, Java filter).
    pub fn over_fields(
        name_type: &'a dyn Fn(&str) -> Option<JType>,
        name_expr: &'a dyn Fn(&str) -> Expr,
    ) -> Lowerer<'a> {
        Lowerer {
            name_type,
            name_expr,
            out: Lowered::exact(Expr::null()),
            js: false,
        }
    }

    /// Lowers `e`; returns its SQL and Java type.
    pub fn lower(&mut self, e: &JExpr) -> Result<(Expr, JType), Gap> {
        Ok(match e {
            JExpr::Name(n) => match (self.name_type)(n) {
                Some(JType::BigDecimal) if self.js => {
                    self.out.note(format!(
                        "`{n}` (BigNumber) is a JavaScript number: arithmetic in double precision"
                    ));
                    (
                        Expr::cast((self.name_expr)(n), KType::Number),
                        JType::Double { boxed: true },
                    )
                }
                Some(t) => ((self.name_expr)(n), t),
                None => return Err(Gap::new(format!("`{n}` is not a field or local variable"))),
            },
            JExpr::Int(i) => (Expr::int(*i), JType::Long { boxed: false }),
            JExpr::Float(f) => (
                Expr::Cast {
                    arg: Box::new(Expr::num(f)),
                    to: KType::Number,
                    length: -1,
                    precision: -1,
                },
                JType::Double { boxed: false },
            ),
            JExpr::Str(s) => (
                if s.is_empty() {
                    Expr::null()
                } else {
                    Expr::str(s)
                },
                JType::Str,
            ),
            JExpr::Char(c) => (Expr::str(&c.to_string()), JType::Str),
            JExpr::Bool(b) => (Expr::bool(*b), JType::Bool { boxed: false }),
            JExpr::Null => (Expr::null(), JType::Null),
            JExpr::Field(target, name) => self.static_field(target, name)?,
            JExpr::Call(target, name, args) => self.call(target.as_deref(), name, args)?,
            JExpr::Unary(op, arg) => {
                let (a, t) = self.lower(arg)?;
                match *op {
                    "!" => (Expr::negate(a), JType::Bool { boxed: false }),
                    "-" if t.numeric() => (
                        Expr::Unary {
                            op: UnOp::Neg,
                            arg: Box::new(a),
                        },
                        unboxed(t),
                    ),
                    "+" if t.numeric() => (a, unboxed(t)),
                    o => return Err(Gap::new(format!("unary `{o}`"))),
                }
            }
            JExpr::Binary(op, l, r) => self.binary(op, l, r)?,
            JExpr::Ternary(c, a, b) => {
                let (c, _) = self.lower(c)?;
                let (a, ta) = self.lower(a)?;
                let (b, tb) = self.lower(b)?;
                let t = if ta == JType::Null { tb } else { ta };
                (Expr::case(vec![(c, a)], Some(b)), t)
            }
            JExpr::Cast(t, arg) => {
                let (a, at) = self.lower(arg)?;
                match t.as_str() {
                    "long" | "int" | "short" | "byte" => {
                        if matches!(at, JType::Double { .. }) {
                            // Java truncates toward zero.
                            let trunc = Expr::case(
                                vec![(
                                    Expr::bin(BinOp::Lt, a.clone(), Expr::int(0)),
                                    Expr::call(Func::Ceil, vec![a.clone()]),
                                )],
                                Some(Expr::call(Func::Floor, vec![a])),
                            );
                            (
                                Expr::cast(trunc, KType::Integer),
                                JType::Long { boxed: false },
                            )
                        } else {
                            (Expr::cast(a, KType::Integer), JType::Long { boxed: false })
                        }
                    }
                    "double" | "float" => {
                        (Expr::cast(a, KType::Number), JType::Double { boxed: false })
                    }
                    "String" | "Object" | "Long" | "Integer" | "Double" | "Boolean" => (a, at),
                    other => return Err(Gap::new(format!("cast to {other}"))),
                }
            }
            JExpr::New(t, _) => return Err(Gap::new(format!("`new {t}`: object construction"))),
            JExpr::Index(..) => return Err(Gap::new("array indexing")),
            JExpr::InstanceOf(_, t) => return Err(Gap::new(format!("instanceof {t}"))),
            JExpr::Assign(..) | JExpr::Postfix(..) => {
                return Err(Gap::new("an assignment inside an expression"));
            }
        })
    }

    fn static_field(&mut self, target: &JExpr, name: &str) -> Result<(Expr, JType), Gap> {
        let is_class = matches!(target, JExpr::Name(n) if (self.name_type)(n).is_none());
        if name == "length" && !is_class {
            let (e, t) = self.lower(target)?;
            if t == JType::Str {
                return Ok((
                    Expr::call(Func::Length, vec![e]),
                    JType::Long { boxed: false },
                ));
            }
        }
        match (target, name) {
            (JExpr::Name(c), "TRUE") if c == "Boolean" => {
                Ok((Expr::bool(true), JType::Bool { boxed: true }))
            }
            (JExpr::Name(c), "FALSE") if c == "Boolean" => {
                Ok((Expr::bool(false), JType::Bool { boxed: true }))
            }
            (JExpr::Name(c), "MAX_VALUE") if c == "Long" => {
                Ok((Expr::int(i64::MAX), JType::Long { boxed: false }))
            }
            (JExpr::Name(c), "MIN_VALUE") if c == "Long" => {
                Ok((Expr::int(i64::MIN), JType::Long { boxed: false }))
            }
            _ => Err(Gap::new(format!("field `{name}` of an object"))),
        }
    }

    fn binary(&mut self, op: &str, l: &JExpr, r: &JExpr) -> Result<(Expr, JType), Gap> {
        // `x == null` and `x != null`.
        if matches!(op, "==" | "!=") && (matches!(l, JExpr::Null) || matches!(r, JExpr::Null)) {
            let other = if matches!(l, JExpr::Null) { r } else { l };
            let (e, _) = self.lower(other)?;
            let b = JType::Bool { boxed: false };
            return Ok((
                if op == "==" {
                    Expr::is_null(e)
                } else {
                    Expr::is_not_null(e)
                },
                b,
            ));
        }
        let (le, lt) = self.lower(l)?;
        let (re, rt) = self.lower(r)?;
        let b = JType::Bool { boxed: false };
        // JavaScript turns a null into 0 in arithmetic and ordering (`null + 42` is 42,
        // `null < 5` is true): a numeric field that may be null reads as coalesce(x, 0).
        let numeric_op = matches!(op, "+" | "-" | "*" | "/" | "%" | "<" | "<=" | ">" | ">=");
        let (le, re) = if self.js && numeric_op && lt.numeric() && rt.numeric() {
            let zero = |e: Expr, t: JType| {
                if t.boxed() {
                    Expr::call(Func::Coalesce, vec![e, Expr::int(0)])
                } else {
                    e
                }
            };
            if lt.boxed() || rt.boxed() {
                self.out
                    .note("JavaScript reads a null number as 0 in arithmetic and comparisons");
            }
            (zero(le, lt), zero(re, rt))
        } else {
            (le, re)
        };
        if self.js {
            match op {
                "==" | "===" | "!=" | "!==" => {
                    let same = lt == rt
                        || (lt.numeric() && rt.numeric())
                        || matches!((lt, rt), (JType::Bool { .. }, JType::Bool { .. }));
                    let known = !matches!(lt, JType::Unknown | JType::Null)
                        && !matches!(rt, JType::Unknown | JType::Null);
                    if !same && op.len() == 2 && known {
                        return Err(Gap::new(format!(
                            "loose `{op}` between {lt:?} and {rt:?} (JavaScript coerces the types)"
                        )));
                    }
                    let eq = Expr::bin(BinOp::NotDistinct, le, re);
                    return Ok((
                        if op.starts_with('!') {
                            Expr::negate(eq)
                        } else {
                            eq
                        },
                        b,
                    ));
                }
                "/" if lt.numeric() && rt.numeric() => {
                    let e = Expr::bin(BinOp::Div, Expr::cast(le, KType::Number), re);
                    return Ok((e, JType::Double { boxed: false }));
                }

                _ => {}
            }
        }
        Ok(match op {
            "+" if lt == JType::Str || rt == JType::Str => {
                self.out.note(NOTE_JAVA_CONCAT);
                let part = |src: &JExpr, e: Expr, t: JType| {
                    // `"" + x`: the empty literal is text here, not Kettle's empty-is-null.
                    if matches!(src, JExpr::Str(s) if s.is_empty()) {
                        return Expr::str("");
                    }
                    let s = if t == JType::Str {
                        e
                    } else {
                        Expr::cast(e, KType::String)
                    };
                    Expr::call(Func::Coalesce, vec![s, Expr::str("null")])
                };
                (
                    Expr::call(Func::Concat, vec![part(l, le, lt), part(r, re, rt)]),
                    JType::Str,
                )
            }
            "+" | "-" | "*" | "/" | "%" => {
                if !(lt.numeric() && rt.numeric()) {
                    return Err(Gap::new(format!("`{op}` on {lt:?} and {rt:?}")));
                }
                if (lt.boxed() || rt.boxed()) && !self.js {
                    self.out.note(NOTE_NPE);
                }
                let t = if matches!(lt, JType::Double { .. }) || matches!(rt, JType::Double { .. })
                {
                    JType::Double { boxed: false }
                } else {
                    JType::Long { boxed: false }
                };
                let bop = match op {
                    "+" => BinOp::Add,
                    "-" => BinOp::Sub,
                    "*" => BinOp::Mul,
                    "/" => BinOp::Div,
                    _ => BinOp::Mod,
                };
                (Expr::bin(bop, le, re), t)
            }
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                let both_objects = lt.boxed() && rt.boxed();
                if matches!(op, "==" | "!=") && both_objects && !self.js {
                    return Err(Gap::new(
                        "`==` between two objects compares identity in Java, not value; \
                         its result depends on the JVM",
                    ));
                }
                if !(lt.numeric() && rt.numeric())
                    && !matches!((lt, rt), (JType::Bool { .. }, JType::Bool { .. }))
                {
                    return Err(Gap::new(format!("`{op}` on {lt:?} and {rt:?}")));
                }
                if (lt.boxed() || rt.boxed()) && !self.js {
                    self.out.note(NOTE_NPE);
                }
                let bop = match op {
                    "==" => BinOp::Eq,
                    "!=" => BinOp::NotEq,
                    "<" => BinOp::Lt,
                    "<=" => BinOp::LtEq,
                    ">" => BinOp::Gt,
                    _ => BinOp::GtEq,
                };
                (Expr::bin(bop, le, re), b)
            }
            "&&" => (Expr::and(le, re), b),
            "||" => (Expr::or(le, re), b),
            o => return Err(Gap::new(format!("operator `{o}`"))),
        })
    }

    fn call(
        &mut self,
        target: Option<&JExpr>,
        name: &str,
        args: &[JExpr],
    ) -> Result<(Expr, JType), Gap> {
        let b = JType::Bool { boxed: false };
        let long = JType::Long { boxed: false };
        let dbl = JType::Double { boxed: false };
        // Static calls on a class name.
        if let Some(JExpr::Name(class)) = target
            && (self.name_type)(class).is_none()
        {
            let mut a = Vec::new();
            for x in args {
                a.push(self.lower(x)?);
            }
            let arg = |i: usize| {
                a.get(i)
                    .map(|x| x.0.clone())
                    .ok_or_else(|| Gap::new(format!("{class}.{name}: missing argument")))
            };
            return Ok(match (class.as_str(), name) {
                ("Math", "abs") => (Expr::call(Func::Abs, vec![arg(0)?]), unboxed(a[0].1)),
                ("Math", "max") => (
                    Expr::call(Func::Greatest, vec![arg(0)?, arg(1)?]),
                    unboxed(a[0].1),
                ),
                ("Math", "min") => (
                    Expr::call(Func::Least, vec![arg(0)?, arg(1)?]),
                    unboxed(a[0].1),
                ),
                ("Math", "floor") => (
                    Expr::cast(Expr::call(Func::Floor, vec![arg(0)?]), KType::Number),
                    dbl,
                ),
                ("Math", "ceil") => (
                    Expr::cast(Expr::call(Func::Ceil, vec![arg(0)?]), KType::Number),
                    dbl,
                ),
                ("Math", "sqrt") => (Expr::call(Func::Sqrt, vec![arg(0)?]), dbl),
                ("Math", "pow") => (Expr::call(Func::Power, vec![arg(0)?, arg(1)?]), dbl),
                ("Math", "round") => {
                    // Java rounds half up (toward positive infinity): floor(x + 0.5).
                    let e = Expr::call(
                        Func::Floor,
                        vec![Expr::bin(BinOp::Add, arg(0)?, Expr::num("0.5"))],
                    );
                    (Expr::cast(e, KType::Integer), long)
                }
                ("String", "valueOf") => {
                    let (e, t) = a
                        .first()
                        .cloned()
                        .ok_or_else(|| Gap::new("String.valueOf()"))?;
                    let s = if t == JType::Str {
                        e
                    } else {
                        Expr::cast(e, KType::String)
                    };
                    (
                        Expr::call(Func::Coalesce, vec![s, Expr::str("null")]),
                        JType::Str,
                    )
                }
                // Boxing a number (`Long.valueOf(qty * 2)`, as javac writes autoboxing): the value.
                ("Integer" | "Long" | "Double", "valueOf")
                    if a.first().is_some_and(|x| x.1.numeric()) =>
                {
                    let (e, t) = a[0].clone();
                    let boxed = match t {
                        JType::Double { .. } => JType::Double { boxed: true },
                        _ => JType::Long { boxed: true },
                    };
                    (e, boxed)
                }
                ("Integer" | "Long", "parseInt" | "parseLong" | "valueOf") => {
                    self.out.note("a value that is not a number would stop the Kettle step; the SQL CAST fails the query the same way");
                    (
                        Expr::cast(Expr::call(Func::Trim, vec![arg(0)?]), KType::Integer),
                        JType::Long {
                            boxed: name == "valueOf",
                        },
                    )
                }
                ("Double", "parseDouble" | "valueOf") => (
                    Expr::cast(Expr::call(Func::Trim, vec![arg(0)?]), KType::Number),
                    JType::Double {
                        boxed: name == "valueOf",
                    },
                ),
                ("Boolean", "valueOf") => (arg(0)?, JType::Bool { boxed: true }),
                (c, m) => return Err(Gap::new(format!("static call {c}.{m}(…)"))),
            });
        }
        let Some(target) = target else {
            if self.js {
                return self.js_function(name, args);
            }
            return Err(Gap::new(format!("call to `{name}(…)`")));
        };
        let (recv, rt) = self.lower(target)?;
        let mut a = Vec::new();
        for x in args {
            a.push(self.lower(x)?);
        }
        let arg = |i: usize| {
            a.get(i)
                .map(|x| x.0.clone())
                .ok_or_else(|| Gap::new(format!(".{name}(): missing argument")))
        };
        if rt.boxed() {
            self.out.note(if self.js {
                "a method called on a null value stops the Kettle step (TypeError); the SQL yields null"
            } else {
                NOTE_NPE
            });
        }
        Ok(match (rt, name, a.len()) {
            (JType::Str, "equals", 1) => (Expr::two_valued(Expr::bin(BinOp::Eq, recv, arg(0)?)), b),
            (JType::Str, "equalsIgnoreCase", 1) => (
                Expr::two_valued(Expr::bin(
                    BinOp::Eq,
                    Expr::call(Func::Lower, vec![recv]),
                    Expr::call(Func::Lower, vec![arg(0)?]),
                )),
                b,
            ),
            (JType::Str, "length", 0) => (Expr::call(Func::Length, vec![recv]), long),
            (JType::Str, "isEmpty", 0) => (
                Expr::bin(
                    BinOp::Eq,
                    Expr::call(Func::Length, vec![recv]),
                    Expr::int(0),
                ),
                b,
            ),
            (JType::Str, "toUpperCase", 0) => (Expr::call(Func::Upper, vec![recv]), JType::Str),
            (JType::Str, "toLowerCase", 0) => (Expr::call(Func::Lower, vec![recv]), JType::Str),
            (JType::Str, "trim", 0) => (Expr::call(Func::Trim, vec![recv]), JType::Str),
            (JType::Str, "substring", 1) => (
                Expr::call(
                    Func::Substr,
                    vec![recv, Expr::bin(BinOp::Add, arg(0)?, Expr::int(1))],
                ),
                JType::Str,
            ),
            (JType::Str, "substring", 2) => (
                Expr::call(
                    Func::Substr,
                    vec![
                        recv,
                        Expr::bin(BinOp::Add, arg(0)?, Expr::int(1)),
                        Expr::bin(BinOp::Sub, arg(1)?, arg(0)?),
                    ],
                ),
                JType::Str,
            ),
            (JType::Str, "startsWith", 1) => (Expr::call(Func::StartsWith, vec![recv, arg(0)?]), b),
            (JType::Str, "endsWith", 1) => {
                let s = arg(0)?;
                let tail = Expr::call(
                    Func::Substr,
                    vec![
                        recv,
                        Expr::Unary {
                            op: UnOp::Neg,
                            arg: Box::new(Expr::call(Func::Length, vec![s.clone()])),
                        },
                    ],
                );
                (Expr::bin(BinOp::Eq, tail, s), b)
            }
            (JType::Str, "contains", 1) => (
                Expr::bin(
                    BinOp::Gt,
                    Expr::call(Func::Strpos, vec![recv, arg(0)?]),
                    Expr::int(0),
                ),
                b,
            ),
            (JType::Str, "indexOf", 1) => (
                Expr::bin(
                    BinOp::Sub,
                    Expr::call(Func::Strpos, vec![recv, arg(0)?]),
                    Expr::int(1),
                ),
                long,
            ),
            (JType::Str, "replace", 2) if self.js => {
                // JavaScript replaces the first occurrence only (for a string pattern).
                let (
                    Expr::Lit {
                        value: crate::expr::Lit::Str(from),
                    },
                    Expr::Lit {
                        value: crate::expr::Lit::Str(to),
                    },
                ) = (&a[0].0, &a[1].0)
                else {
                    return Err(Gap::new("String.replace with computed arguments"));
                };
                let mut pat = String::from("(?s)^(.*?)");
                for c in from.chars() {
                    if "\\.^$|?*+()[]{}".contains(c) {
                        pat.push('\\');
                    }
                    pat.push(c);
                }
                let by = format!("$1{}", to.replace('$', "\\$"));
                (
                    Expr::call(
                        Func::RegexpReplace,
                        vec![recv, Expr::str(&pat), Expr::str(&by)],
                    ),
                    JType::Str,
                )
            }
            (JType::Str, "replace", 2) => (
                Expr::call(Func::Replace, vec![recv, arg(0)?, arg(1)?]),
                JType::Str,
            ),
            (JType::Str, "concat", 1) => {
                (Expr::call(Func::Concat, vec![recv, arg(0)?]), JType::Str)
            }
            (JType::Str, "matches", 1) => {
                self.out.note(NOTE_REGEX_FULL);
                let p = match &a[0].0 {
                    Expr::Lit {
                        value: crate::expr::Lit::Str(s),
                    } => Expr::str(&full_match(s)),
                    other => Expr::call(
                        Func::Concat,
                        vec![Expr::str("^(?:"), other.clone(), Expr::str(")$")],
                    ),
                };
                (Expr::call(Func::RegexpLike, vec![recv, p]), b)
            }
            (JType::Long { .. }, "longValue" | "intValue", 0) => (recv, long),
            (JType::Long { .. } | JType::Double { .. }, "doubleValue", 0) => {
                (Expr::cast(recv, KType::Number), dbl)
            }
            (JType::Bool { .. }, "booleanValue", 0) => (recv, b),
            (_, "toString", 0) => (Expr::cast(recv, KType::String), JType::Str),
            (t, m, n) => return Err(Gap::new(format!("method {m}/{n} on {t:?}"))),
        })
    }
}

impl Lowerer<'_> {
    /// Kettle's JavaScript built-ins and the global JavaScript functions (RFC 0203).
    fn js_function(&mut self, name: &str, args: &[JExpr]) -> Result<(Expr, JType), Gap> {
        let mut a = Vec::new();
        for x in args {
            a.push(self.lower(x)?);
        }
        let arg = |i: usize| {
            a.get(i)
                .map(|x| x.0.clone())
                .ok_or_else(|| Gap::new(format!("{name}(): missing argument")))
        };
        let s = JType::Str;
        let long = JType::Long { boxed: false };
        let dbl = JType::Double { boxed: false };
        Ok(match (name, a.len()) {
            ("upper", 1) => (Expr::call(Func::Upper, vec![arg(0)?]), s),
            ("lower", 1) => (Expr::call(Func::Lower, vec![arg(0)?]), s),
            ("trim", 1) => (Expr::call(Func::Trim, vec![arg(0)?]), s),
            ("ltrim", 1) => (Expr::call(Func::LTrim, vec![arg(0)?]), s),
            ("rtrim", 1) => (Expr::call(Func::RTrim, vec![arg(0)?]), s),
            ("isNull", 1) => (Expr::is_null(arg(0)?), JType::Bool { boxed: false }),
            ("replace", 3) => (
                Expr::call(Func::Replace, vec![arg(0)?, arg(1)?, arg(2)?]),
                s,
            ),
            ("indexOf", 2) => (
                Expr::bin(
                    BinOp::Sub,
                    Expr::call(Func::Strpos, vec![arg(0)?, arg(1)?]),
                    Expr::int(1),
                ),
                long,
            ),
            ("getDigitsOnly", 1) => (
                Expr::call(
                    Func::RegexpReplace,
                    vec![arg(0)?, Expr::str("[^0-9]"), Expr::str("")],
                ),
                s,
            ),
            ("removeDigits", 1) => (
                Expr::call(
                    Func::RegexpReplace,
                    vec![arg(0)?, Expr::str("[0-9]"), Expr::str("")],
                ),
                s,
            ),
            ("abs", 1) => (Expr::call(Func::Abs, vec![arg(0)?]), dbl),
            ("ceil", 1) => (
                Expr::cast(Expr::call(Func::Ceil, vec![arg(0)?]), KType::Number),
                dbl,
            ),
            ("floor", 1) => (
                Expr::cast(Expr::call(Func::Floor, vec![arg(0)?]), KType::Number),
                dbl,
            ),
            ("round", 1) => (
                Expr::cast(
                    Expr::call(
                        Func::Floor,
                        vec![Expr::bin(BinOp::Add, arg(0)?, Expr::num("0.5"))],
                    ),
                    KType::Number,
                ),
                dbl,
            ),
            ("str2num", 1) | ("Number", 1) | ("parseFloat", 1) => {
                if name == "parseFloat" {
                    self.out.note("parseFloat reads a leading number (\"12abc\" is 12); the SQL CAST fails on trailing text");
                }
                (
                    Expr::cast(Expr::call(Func::Trim, vec![arg(0)?]), KType::Number),
                    dbl,
                )
            }
            ("parseInt", 1) => {
                self.out.note("parseInt reads a leading integer (\"12abc\" is 12); the SQL CAST fails on trailing text");
                (
                    Expr::cast(Expr::call(Func::Trim, vec![arg(0)?]), KType::Integer),
                    long,
                )
            }
            ("num2str", 1) | ("String", 1) => {
                let (e, t) = a[0].clone();
                let e = if t == JType::Str {
                    e
                } else {
                    Expr::cast(e, KType::String)
                };
                (Expr::call(Func::Coalesce, vec![e, Expr::str("null")]), s)
            }
            ("str2date", 2) => (
                Expr::call(Func::ParseDatetime, vec![arg(0)?, arg(1)?]),
                JType::Date,
            ),
            ("date2str", 2) => (Expr::call(Func::FormatDatetime, vec![arg(0)?, arg(1)?]), s),
            (other, n) => return Err(Gap::new(format!("JavaScript function {other}/{n}"))),
        })
    }
}

fn unboxed(t: JType) -> JType {
    match t {
        JType::Long { .. } => JType::Long { boxed: false },
        JType::Double { .. } => JType::Double { boxed: false },
        JType::Bool { .. } => JType::Bool { boxed: false },
        other => other,
    }
}

/// Lowers a Java expression over Kettle fields of the given types.
pub fn lower_over_fields(
    src: &str,
    field_type: &dyn Fn(&str) -> Option<KType>,
) -> Result<(Lowered, KType), Gap> {
    let e = parse_expr(src)?;
    let nt = |n: &str| field_type(n).map(JType::of_field);
    let ne = |n: &str| Expr::col(n);
    let mut l = Lowerer::over_fields(&nt, &ne);
    let (expr, t) = l.lower(&e)?;
    let mut out = l.out;
    out.expr = expr;
    Ok((out, t.ktype()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantics::Fidelity;

    fn ft(n: &str) -> Option<KType> {
        match n {
            "FieldName" | "StructureName" | "Coordinates" | "name" => Some(KType::String),
            "qty" => Some(KType::Integer),
            "FRIEND" => Some(KType::Boolean),
            _ => None,
        }
    }

    #[test]
    fn the_corpus_expressions() {
        let (l, t) = lower_over_fields(
            "(FieldName == null) ? StructureName+Coordinates : FieldName",
            &ft,
        )
        .unwrap();
        assert_eq!(t, KType::String);
        assert_eq!(
            l.expr.sql(),
            "CASE WHEN (\"FieldName\" IS NULL) THEN concat(coalesce(\"StructureName\", 'null'), coalesce(\"Coordinates\", 'null')) ELSE \"FieldName\" END"
        );
        assert_eq!(l.fidelity, Fidelity::Rewritten);
        let (l, _) = lower_over_fields("(FRIEND) ?  Boolean.TRUE : Boolean.FALSE", &ft).unwrap();
        assert_eq!(
            l.expr.sql(),
            "CASE WHEN \"FRIEND\" THEN TRUE ELSE FALSE END"
        );
        assert!(
            lower_over_fields("org.pentaho.di.core.Const.getPercentageFreeMemory()", &ft).is_err()
        );
    }

    #[test]
    fn java_semantics_are_kept() {
        let (l, _) = lower_over_fields("name.substring(1, 3).toUpperCase()", &ft).unwrap();
        assert_eq!(l.expr.sql(), "upper(substr(\"name\", (1 + 1), (3 - 1)))");
        let (l, _) = lower_over_fields("qty * 2 > 10 && name.equals(\"x\")", &ft).unwrap();
        assert_eq!(
            l.expr.sql(),
            "(((\"qty\" * 2) > 10) AND coalesce((\"name\" = 'x'), FALSE))"
        );
        let gap = lower_over_fields("name == FieldName", &ft).unwrap_err();
        assert!(gap.reason.contains("identity"), "{gap}");
        assert!(lower_over_fields("unknownField + 1", &ft).is_err());
        let (l, _) = lower_over_fields("(long) Math.round(2.5)", &ft).unwrap();
        assert!(l.expr.sql().contains("floor"));
    }

    #[test]
    fn the_lexer_handles_comments_literals_and_operators() {
        let t = lex("a >>>= 0x1F; // c\n /* x */ 'c' \"s\\\"q\" 1.5f 10L").unwrap();
        assert_eq!(
            t,
            vec![
                Tok::Ident("a".into()),
                Tok::P(">>>="),
                Tok::Int(31),
                Tok::P(";"),
                Tok::Char('c'),
                Tok::Str("s\"q".into()),
                Tok::Float("1.5".into()),
                Tok::Int(10)
            ]
        );
    }
}

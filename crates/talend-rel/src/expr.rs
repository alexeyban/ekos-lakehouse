//! Talend's Java expressions as typed SQL (EKOS RFC 0267 §Expressions).
//!
//! The text is parsed by `etl-expr`'s Java parser (RFC 0266). This lowering resolves Talend's
//! names (`row1.COL`, `Var.x`, `context.x`) and applies Talend's rules, which are plain Java's
//! (no empty-string-is-null, unlike Kettle). Every rule below `exact` is named in the result's
//! notes; anything outside the grammar or the closed routine and method lists is a gap with its
//! text.

use ekos_lakehouse_etl_expr::java::{JExpr, parse_expr};
use ekos_lakehouse_etl_expr::{BinOp, Expr, Fidelity, Func, Gap, KType, Lit, Lowered, UnOp, Unit};

pub const NOTE_EQUALS_NULL: &str = "`a.equals(b)` stops the Talend job when `a` is null; SQL compares \
     null as a value (IS NOT DISTINCT FROM)";
pub const NOTE_REFERENCE_EQUALITY: &str = "`==` on strings or boxed values compares references in \
     Java; lowered as value equality — read the original";
pub const NOTE_NULL_CONCAT: &str =
    "Java string concatenation prints a null operand as `null`; kept as written";
pub const NOTE_CONTEXT: &str =
    "a context variable is lowered to its default value; a run may replace it";
pub const NOTE_RUN_START: &str =
    "`TalendDate.getCurrentDate()` is the clock at each row; lowered to the run's start time";
pub const NOTE_NPE: &str = "a method called on a null value stops the Talend job \
     (NullPointerException); the SQL yields null";
pub const NOTE_PLATFORM_CHARSET: &str = "the routine reads the text's bytes in the platform's \
     default charset; the translation hashes UTF-8 (the same for ASCII text)";
pub const NOTE_MONTH_ZERO: &str =
    "`getPartOfDate(\"MONTH\", …)` is zero-based (January = 0), as Java's Calendar";
const NOTE_ISO_WEEK: &str = "`getPartOfDate(\"WEEK_OF_YEAR\", …)` is the ISO week, as Java's under the Monday-first, four-day locale the operator stated";

/// `<qualifier>.<column>` → the column's SQL and type.
pub type ColumnLookup<'a> = dyn Fn(&str, &str) -> Option<(Expr, KType)> + 'a;
/// `context.<name>` → its default value (`None` when unset) and type.
pub type ContextLookup<'a> = dyn Fn(&str) -> Option<(Option<String>, KType)> + 'a;
/// `Numeric.sequence("<name>", …)` → the column that numbers the rows for it.
pub type SequenceLookup<'a> = dyn Fn(&str) -> Option<Expr> + 'a;
/// `globalMap.get("<key>")` → what it stands for, its type and the note that says how.
pub type GlobalLookup<'a> = dyn Fn(&str) -> Option<(Expr, KType, String)> + 'a;

/// What names mean where the expression is written.
pub struct Scope<'a> {
    /// `<qualifier>.<column>`: the column's SQL and type, `None` when the qualifier or column is
    /// unknown.
    pub column: &'a ColumnLookup<'a>,
    /// `Var.<name>`: the variable's expression text (a tMap's variable table).
    pub var: &'a dyn Fn(&str) -> Option<String>,
    /// `context.<name>`: the default value (`None` when unset) and its type; `None` when the job
    /// has no such variable.
    pub context: &'a ContextLookup<'a>,
    /// The job's name: Talend's `jobName` is this job's own name.
    pub job: &'a str,
    /// `globalMap.get("<key>")`: what the key stands for, and the note that says how, when the
    /// lowering knows (a `tSetGlobalVar` constant, a `tFileList`'s current file).
    pub global: &'a GlobalLookup<'a>,
    /// The project's routines: a custom one whose body is a digest is lowered
    /// ([`crate::routine::digest`]).
    pub routines: &'a [ekos_talend::Routine],
    /// The JVM's locale, when the operator stated it ([`crate::Options::locale`]).
    pub locale: Option<crate::lower::Locale>,
    /// `Numeric.sequence("<name>", …)`: the column numbering the rows, when the component
    /// computed one (RFC 0267 phase 2 §Row order).
    pub sequence: &'a SequenceLookup<'a>,
}

/// Talend's type id to a type (`id_String` → string, …); unknown ids are [`KType::Unknown`].
pub fn talend_type(id: Option<&str>) -> KType {
    match id.unwrap_or_default() {
        "id_String" | "id_Character" => KType::String,
        "id_Integer" | "id_Long" | "id_Short" | "id_Byte" => KType::Integer,
        "id_Float" | "id_Double" => KType::Number,
        "id_BigDecimal" => KType::BigNumber,
        "id_Boolean" => KType::Boolean,
        "id_Date" => KType::Timestamp,
        _ => KType::Unknown,
    }
}

/// Lowers one Java expression.
pub fn lower(src: &str, scope: &Scope<'_>) -> Result<(Lowered, KType), Gap> {
    let e = parse_expr(src)?;
    let mut l = Lowerer {
        scope,
        out: Lowered::exact(Expr::null()),
        depth: 0,
    };
    let (expr, ty) = l.lower(&e)?;
    let mut out = l.out;
    out.expr = expr;
    Ok((out, ty))
}

/// Lowers one already parsed Java expression (a statement's right-hand side).
pub fn lower_parsed(e: &JExpr, scope: &Scope<'_>) -> Result<(Lowered, KType), Gap> {
    let mut l = Lowerer {
        scope,
        out: Lowered::exact(Expr::null()),
        depth: 0,
    };
    let (expr, ty) = l.lower(e)?;
    let mut out = l.out;
    out.expr = expr;
    Ok((out, ty))
}

struct Lowerer<'s, 'a> {
    scope: &'s Scope<'a>,
    out: Lowered,
    /// Variables inline into one another: a cycle would never end.
    depth: usize,
}

type Typed = (Expr, KType);

fn is_text(t: KType) -> bool {
    t == KType::String
}

fn numeric(t: KType) -> bool {
    matches!(t, KType::Integer | KType::Number | KType::BigNumber)
}

fn gap<T>(text: impl Into<String>) -> Result<T, Gap> {
    Err(Gap::new(text))
}

impl Lowerer<'_, '_> {
    fn lower(&mut self, e: &JExpr) -> Result<Typed, Gap> {
        Ok(match e {
            JExpr::Int(i) => (Expr::int(*i), KType::Integer),
            JExpr::Float(f) => (
                Expr::num(f.trim_end_matches(['d', 'D', 'f', 'F'])),
                KType::Number,
            ),
            JExpr::Str(s) => (Expr::str(s), KType::String),
            JExpr::Char(c) => (Expr::str(&c.to_string()), KType::String),
            JExpr::Bool(b) => (Expr::bool(*b), KType::Boolean),
            JExpr::Null => (Expr::null(), KType::Unknown),
            JExpr::Name(n) if n == "jobName" => (Expr::str(self.scope.job), KType::String),
            JExpr::Name(n) => return gap(format!("unknown name `{n}`")),
            JExpr::Field(target, member) => self.field(target, member)?,
            JExpr::Call(Some(recv), name, args) => self.call(recv, name, args)?,
            JExpr::Call(None, name, _) => {
                return gap(format!("call to `{name}(…)` without a receiver"));
            }
            JExpr::Unary(op, arg) => {
                let (a, t) = self.lower(arg)?;
                match *op {
                    "!" => (
                        Expr::Unary {
                            op: UnOp::Not,
                            arg: Box::new(a),
                        },
                        KType::Boolean,
                    ),
                    "-" => (
                        Expr::Unary {
                            op: UnOp::Neg,
                            arg: Box::new(a),
                        },
                        t,
                    ),
                    "+" => (a, t),
                    other => return gap(format!("unary `{other}`")),
                }
            }
            JExpr::Binary(op, l, r) => self.binary(op, l, r)?,
            JExpr::Ternary(c, a, b) => {
                let (c, _) = self.lower(c)?;
                let (a, ta) = self.lower(a)?;
                let (b, tb) = self.lower(b)?;
                let ty = if ta == KType::Unknown { tb } else { ta };
                (Expr::case(vec![(Expr::two_valued(c), a)], Some(b)), ty)
            }
            JExpr::Cast(to, arg) => {
                let (a, t) = self.lower(arg)?;
                match to.as_str() {
                    "int" | "long" | "Integer" | "Long" | "short" | "Short" => {
                        let to = KType::Integer;
                        (if t == to { a } else { Expr::cast(a, to) }, to)
                    }
                    "double" | "float" | "Double" | "Float" => {
                        (Expr::cast(a, KType::Number), KType::Number)
                    }
                    "String" => (a, t),
                    other => return gap(format!("cast to `{other}`")),
                }
            }
            other => return gap(format!("Java construct not translated: {other:?}")),
        })
    }

    fn field(&mut self, target: &JExpr, member: &str) -> Result<Typed, Gap> {
        let JExpr::Name(q) = target else {
            return gap(format!("field `{member}` of a computed value"));
        };
        match q.as_str() {
            "context" => {
                let Some((value, ty)) = (self.scope.context)(member) else {
                    return gap(format!("context variable `{member}` is not defined"));
                };
                self.out.note(NOTE_CONTEXT);
                Ok(match value {
                    None => (Expr::null(), ty),
                    Some(v) => (context_literal(&v, ty), ty),
                })
            }
            "Var" => {
                let Some(text) = (self.scope.var)(member) else {
                    return gap(format!("tMap variable `Var.{member}` is not defined"));
                };
                if self.depth > 16 {
                    return gap(format!("tMap variable `Var.{member}` refers to itself"));
                }
                self.depth += 1;
                let e = parse_expr(&text)?;
                let r = self.lower(&e);
                self.depth -= 1;
                r
            }
            _ => (self.scope.column)(q, member)
                .map_or_else(|| gap(format!("`{q}.{member}` is not an input column")), Ok),
        }
    }

    fn args(&mut self, args: &[JExpr]) -> Result<Vec<Typed>, Gap> {
        args.iter().map(|a| self.lower(a)).collect()
    }

    fn call(&mut self, recv: &JExpr, name: &str, args: &[JExpr]) -> Result<Typed, Gap> {
        if matches!(recv, JExpr::Name(n) if n == "globalMap") {
            let key = match args.first() {
                Some(JExpr::Str(k)) => k.clone(),
                _ => "…".into(),
            };
            if let Some((e, t, note)) = (self.scope.global)(&key) {
                self.out.note(note);
                return Ok((e, t));
            }
            return gap(format!("run-time value globalMap.get(\"{key}\")"));
        }
        if let JExpr::Name(class) = recv
            && (self.scope.column)(class, name).is_none()
            && class.starts_with(|c: char| c.is_ascii_uppercase())
        {
            return self.routine(class, name, args);
        }
        if let JExpr::Field(inner, class) = recv
            && matches!(inner.as_ref(), JExpr::Name(r) if r == "routines")
        {
            return self.routine(class, name, args);
        }
        let (r, rt) = self.lower(recv)?;
        let a = self.args(args)?;
        let arg = |i: usize| a[i].0.clone();
        if !matches!(r, Expr::Lit { .. }) {
            self.out.note(NOTE_NPE);
        }
        let b = KType::Boolean;
        Ok(match (name, a.len()) {
            ("equals", 1) => {
                self.out.note(NOTE_EQUALS_NULL);
                (Expr::bin(BinOp::NotDistinct, r, arg(0)), b)
            }
            ("equalsIgnoreCase", 1) => {
                self.out.note(NOTE_EQUALS_NULL);
                (
                    Expr::bin(
                        BinOp::NotDistinct,
                        Expr::call(Func::Lower, vec![r]),
                        Expr::call(Func::Lower, vec![arg(0)]),
                    ),
                    b,
                )
            }
            ("compareTo", 1) => (
                Expr::case(
                    vec![
                        (Expr::bin(BinOp::Lt, r.clone(), arg(0)), Expr::int(-1)),
                        (Expr::bin(BinOp::Gt, r, arg(0)), Expr::int(1)),
                    ],
                    Some(Expr::int(0)),
                ),
                KType::Integer,
            ),
            ("length", 0) => (Expr::call(Func::Length, vec![r]), KType::Integer),
            ("isEmpty", 0) => (
                Expr::bin(BinOp::Eq, Expr::call(Func::Length, vec![r]), Expr::int(0)),
                b,
            ),
            ("trim", 0) => (Expr::call(Func::Trim, vec![r]), KType::String),
            ("toUpperCase", 0) => (Expr::call(Func::Upper, vec![r]), KType::String),
            ("toLowerCase", 0) => (Expr::call(Func::Lower, vec![r]), KType::String),
            ("substring", 1) => (
                Expr::call(
                    Func::Substr,
                    vec![r, Expr::bin(BinOp::Add, arg(0), Expr::int(1))],
                ),
                KType::String,
            ),
            ("substring", 2) => (
                Expr::call(
                    Func::Substr,
                    vec![
                        r,
                        Expr::bin(BinOp::Add, arg(0), Expr::int(1)),
                        Expr::bin(BinOp::Sub, arg(1), arg(0)),
                    ],
                ),
                KType::String,
            ),
            ("contains", 1) => (
                Expr::bin(
                    BinOp::Gt,
                    Expr::call(Func::Strpos, vec![r, arg(0)]),
                    Expr::int(0),
                ),
                b,
            ),
            ("startsWith", 1) => (Expr::call(Func::StartsWith, vec![r, arg(0)]), b),
            ("endsWith", 1) => {
                let s = arg(0);
                let tail = Expr::call(
                    Func::Substr,
                    vec![
                        r,
                        Expr::Unary {
                            op: UnOp::Neg,
                            arg: Box::new(Expr::call(Func::Length, vec![s.clone()])),
                        },
                    ],
                );
                (Expr::bin(BinOp::Eq, tail, s), b)
            }
            ("replace", 2) => (
                Expr::call(Func::Replace, vec![r, arg(0), arg(1)]),
                KType::String,
            ),
            ("toString", 0) => (Expr::cast(r, KType::String), KType::String),
            ("intValue" | "longValue" | "shortValue", 0) => {
                if rt != KType::Integer {
                    self.out
                        .note("`intValue()` truncates toward zero, as Java does");
                }
                (Expr::cast(r, KType::Integer), KType::Integer)
            }
            ("doubleValue" | "floatValue", 0) => (Expr::cast(r, KType::Number), KType::Number),
            ("matches", 1) => {
                let Expr::Lit {
                    value: Lit::Str(re),
                } = &a[0].0
                else {
                    return gap("String.matches with a computed pattern");
                };
                self.out.note(
                    "Java's `matches` must match the whole string; the pattern is anchored, and Java and Trino regular expressions differ in rare constructs",
                );
                (
                    Expr::call(Func::RegexpLike, vec![r, Expr::str(&format!("^(?:{re})$"))]),
                    b,
                )
            }
            _ if !is_text(rt) && rt != KType::Unknown => {
                return gap(format!("method `{name}` on a {rt:?} value"));
            }
            _ => return gap(format!("method `{name}` with {} argument(s)", a.len())),
        })
    }

    fn routine(&mut self, class: &str, name: &str, args: &[JExpr]) -> Result<Typed, Gap> {
        // A literal part name (`"MONTH"`) is read before the arguments are lowered.
        let literal = |i: usize| match args.get(i) {
            Some(JExpr::Str(s)) => Some(s.clone()),
            _ => None,
        };
        let a = self.args(args)?;
        let arg = |i: usize| a[i].0.clone();
        let n = a.len();
        Ok(match (class, name, n) {
            ("TalendDate", "getCurrentDate", 0) => {
                self.out.note(NOTE_RUN_START);
                (Expr::call(Func::Now, vec![]), KType::Timestamp)
            }
            ("TalendDate", "getPartOfDate", 2) => {
                let Some(part) = literal(0) else {
                    return gap("TalendDate.getPartOfDate with a computed part");
                };
                let d = arg(1);
                let f = |func| (Expr::call(func, vec![d.clone()]), KType::Integer);
                match part.as_str() {
                    "YEAR" => f(Func::Year),
                    "MONTH" => {
                        self.out.note(NOTE_MONTH_ZERO);
                        (
                            Expr::bin(BinOp::Sub, Expr::call(Func::Month, vec![d]), Expr::int(1)),
                            KType::Integer,
                        )
                    }
                    "DAY_OF_MONTH" => f(Func::Day),
                    "DAY_OF_WEEK" => f(Func::DayOfWeekSunday1),
                    "DAY_OF_YEAR" => f(Func::DayOfYear),
                    "HOUR_OF_DAY" => f(Func::Hour),
                    "MINUTE" => f(Func::Minute),
                    "SECOND" => f(Func::Second),
                    "WEEK_OF_YEAR" => match self.scope.locale {
                        Some(l) if l.iso_weeks => {
                            self.out.note(format!("{NOTE_ISO_WEEK} (`{}`)", l.tag));
                            f(Func::IsoWeek)
                        }
                        Some(l) => {
                            return gap(format!(
                                "TalendDate.getPartOfDate part `WEEK_OF_YEAR` under `{}` counts weeks from Sunday, which has no SQL function here",
                                l.tag
                            ));
                        }
                        None => {
                            return gap(
                                "TalendDate.getPartOfDate part `WEEK_OF_YEAR` follows the JVM's locale: state it (`--locale`)",
                            );
                        }
                    },
                    other => {
                        return gap(format!(
                            "TalendDate.getPartOfDate part `{other}` (locale-dependent or not mapped)"
                        ));
                    }
                }
            }
            ("TalendDate", "addDate", 3) => {
                let Some(unit) = literal(2).as_deref().and_then(date_unit) else {
                    return gap("TalendDate.addDate with a computed or unknown unit");
                };
                (
                    Expr::call(Func::DateAdd(unit), vec![arg(0), arg(1)]),
                    a[0].1,
                )
            }
            ("TalendDate", "diffDate", 3) => {
                let Some(unit) = literal(2).as_deref().and_then(date_unit) else {
                    return gap("TalendDate.diffDate with a computed or unknown unit");
                };
                (
                    Expr::call(Func::DateDiff(unit), vec![arg(0), arg(1)]),
                    KType::Integer,
                )
            }
            // The last (first) day of the date's month, the time of day kept, as Talend's
            // Calendar arithmetic keeps it.
            ("TalendDate", "getLastDayOfMonth" | "getFirstDayOfMonth", 1) => {
                let d = arg(0);
                let day = |e: Expr| Expr::call(Func::Day, vec![e]);
                let target = if name == "getLastDayOfMonth" {
                    let last = Expr::call(
                        Func::DateAdd(Unit::Day),
                        vec![
                            Expr::call(
                                Func::DateAdd(Unit::Month),
                                vec![
                                    Expr::call(Func::DateTrunc(Unit::Month), vec![d.clone()]),
                                    Expr::int(1),
                                ],
                            ),
                            Expr::int(-1),
                        ],
                    );
                    day(last)
                } else {
                    Expr::int(1)
                };
                (
                    Expr::call(
                        Func::DateAdd(Unit::Day),
                        vec![d.clone(), Expr::bin(BinOp::Sub, target, day(d))],
                    ),
                    a[0].1,
                )
            }
            // Both dates printed with a numeric pattern and compared as text: -1, 0 or 1, a null
            // date before any other (Talend's `compareDate`).
            ("TalendDate", "compareDate", 3) => {
                let Some(p) = literal(2).filter(|p| plain_pattern(p)) else {
                    return gap(
                        "TalendDate.compareDate with a computed or locale-dependent pattern",
                    );
                };
                let (d1, d2) = (arg(0), arg(1));
                let fmt = |e: Expr| Expr::call(Func::FormatDatetime, vec![e, Expr::str(&p)]);
                let null = |e: &Expr| Expr::IsNull {
                    arg: Box::new(e.clone()),
                    negated: false,
                };
                (
                    Expr::case(
                        vec![
                            (Expr::bin(BinOp::And, null(&d1), null(&d2)), Expr::int(0)),
                            (null(&d1), Expr::int(-1)),
                            (null(&d2), Expr::int(1)),
                            (
                                Expr::bin(BinOp::Gt, fmt(d1.clone()), fmt(d2.clone())),
                                Expr::int(1),
                            ),
                            (Expr::bin(BinOp::Lt, fmt(d1), fmt(d2)), Expr::int(-1)),
                        ],
                        Some(Expr::int(0)),
                    ),
                    KType::Integer,
                )
            }
            ("TalendDate", "formatDate", 2)
                if matches!(literal(0).as_deref(), Some("MMMM" | "EEEE"))
                    && self.scope.locale.is_some() =>
            {
                let l = self.scope.locale.expect("checked");
                let month = literal(0).as_deref() == Some("MMMM");
                let (func, names, first): (Func, &[&str], i64) = if month {
                    (Func::Month, &l.months[..], 1)
                } else {
                    (Func::DayOfWeekSunday1, &l.days[..], 1)
                };
                self.out.note(format!(
                    "`formatDate(\"{}\", …)` prints the {} name in the JVM's locale; `{}` as the operator stated (CLDR names, Java 9+)",
                    if month { "MMMM" } else { "EEEE" },
                    if month { "month's" } else { "day's" },
                    l.tag
                ));
                let part = Expr::call(func, vec![arg(1)]);
                let when = names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        (
                            Expr::bin(BinOp::Eq, part.clone(), Expr::int(first + i as i64)),
                            Expr::str(n),
                        )
                    })
                    .collect();
                (
                    Expr::Case {
                        when,
                        otherwise: None,
                    },
                    KType::String,
                )
            }
            ("TalendDate", "formatDate", 2) => {
                let Some(p) = literal(0).filter(|p| plain_pattern(p)) else {
                    return gap(
                        "TalendDate.formatDate with a computed or locale-dependent pattern",
                    );
                };
                (
                    Expr::call(Func::FormatDatetime, vec![arg(1), Expr::str(&p)]),
                    KType::String,
                )
            }
            ("TalendDate", "parseDate", 2) => {
                let Some(p) = literal(0).filter(|p| plain_pattern(p)) else {
                    return gap("TalendDate.parseDate with a computed or locale-dependent pattern");
                };
                (
                    Expr::call(Func::ParseDatetime, vec![arg(1), Expr::str(&p)]),
                    KType::Timestamp,
                )
            }
            ("StringHandling", "UPCASE", 1) => {
                (Expr::call(Func::Upper, vec![arg(0)]), KType::String)
            }
            ("StringHandling", "DOWNCASE", 1) => {
                (Expr::call(Func::Lower, vec![arg(0)]), KType::String)
            }
            ("StringHandling", "TRIM", 1) => (Expr::call(Func::Trim, vec![arg(0)]), KType::String),
            ("StringHandling", "LEN", 1) => {
                (Expr::call(Func::Length, vec![arg(0)]), KType::Integer)
            }
            ("StringHandling", "LEFT", 2) => (
                Expr::call(Func::Substr, vec![arg(0), Expr::int(1), arg(1)]),
                KType::String,
            ),
            ("StringHandling", "RIGHT", 2) => (
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
                KType::String,
            ),
            ("StringHandling", "INDEX", 2) => (
                Expr::bin(
                    BinOp::Sub,
                    Expr::call(Func::Strpos, vec![arg(0), arg(1)]),
                    Expr::int(1),
                ),
                KType::Integer,
            ),
            ("StringHandling", "CHANGE", 3) => (
                Expr::call(Func::Replace, vec![arg(0), arg(1), arg(2)]),
                KType::String,
            ),
            ("Relational", "ISNULL", 1) => (Expr::is_null(arg(0)), KType::Boolean),
            ("Mathematical", "ABS", 1) | ("Math", "abs", 1) => {
                (Expr::call(Func::Abs, vec![arg(0)]), a[0].1)
            }
            ("Math", "floor", 1) => (Expr::call(Func::Floor, vec![arg(0)]), KType::Number),
            ("Math", "ceil", 1) => (Expr::call(Func::Ceil, vec![arg(0)]), KType::Number),
            ("Math", "round", 1) => {
                self.out
                    .note("Java's Math.round rounds half up; Trino rounds half away from zero");
                (Expr::call(Func::Round, vec![arg(0)]), KType::Integer)
            }
            ("Integer", "parseInt", 1) | ("Integer", "valueOf", 1) | ("Long", "parseLong", 1) => {
                (Expr::cast(arg(0), KType::Integer), KType::Integer)
            }
            ("Double", "parseDouble", 1) | ("Double", "valueOf", 1) => {
                (Expr::cast(arg(0), KType::Number), KType::Number)
            }
            ("String", "valueOf", 1) => (Expr::cast(arg(0), KType::String), KType::String),
            ("BigDecimal", "valueOf", 1) => {
                (Expr::cast(arg(0), KType::BigNumber), KType::BigNumber)
            }
            ("Numeric", "sequence", _) => {
                if let Some(name) = literal(0)
                    && let Some(col) = (self.scope.sequence)(&name)
                {
                    return Ok((col, KType::Integer));
                }
                return gap("Numeric.sequence depends on the order rows arrive in");
            }
            _ if n == 1
                && let Some(d) = crate::routine::digest(self.scope.routines, class, name) =>
            {
                self.out.note(format!(
                    "routine `{class}.{name}` is a digest in {} hex: lowered to the same digest; read the original",
                    if d.upper { "upper-case" } else { "lower-case" }
                ));
                if !d.null_safe {
                    self.out.note(NOTE_NPE);
                }
                if !d.utf8 {
                    self.out.note(NOTE_PLATFORM_CHARSET);
                }
                let text = if a[0].1 == KType::String {
                    arg(0)
                } else {
                    Expr::cast(arg(0), KType::String)
                };
                let hex = Expr::call(d.func, vec![text]);
                (
                    if d.upper {
                        Expr::call(Func::Upper, vec![hex])
                    } else {
                        hex
                    },
                    KType::String,
                )
            }
            _ => {
                return gap(format!(
                    "routine `{class}.{name}` with {n} argument(s) is not in the catalogue"
                ));
            }
        })
    }

    fn binary(&mut self, op: &str, l: &JExpr, r: &JExpr) -> Result<Typed, Gap> {
        let (le, lt) = self.lower(l)?;
        let (re, rt) = self.lower(r)?;
        let cmp = |o: BinOp| Ok((Expr::bin(o, le.clone(), re.clone()), KType::Boolean));
        match op {
            "+" if is_text(lt) || is_text(rt) => {
                self.out.note(NOTE_NULL_CONCAT);
                let part = |e: Expr| match e {
                    Expr::Lit { .. } => e,
                    other => Expr::call(
                        Func::Coalesce,
                        vec![Expr::cast(other, KType::String), Expr::str("null")],
                    ),
                };
                Ok((
                    Expr::call(Func::Concat, vec![part(le), part(re)]),
                    KType::String,
                ))
            }
            "+" | "-" | "*" | "%" => {
                let o = match op {
                    "+" => BinOp::Add,
                    "-" => BinOp::Sub,
                    "*" => BinOp::Mul,
                    _ => BinOp::Mod,
                };
                Ok((Expr::bin(o, le, re), wider(lt, rt)))
            }
            "/" => {
                let t = wider(lt, rt);
                if t == KType::Integer {
                    self.out.note("integer division truncates, as in Java");
                }
                Ok((Expr::bin(BinOp::Div, le, re), t))
            }
            "==" | "!=" => {
                if is_text(lt) || is_text(rt) {
                    self.out.note(NOTE_REFERENCE_EQUALITY);
                }
                let null_test = |e: &Expr| matches!(e, Expr::Lit { value: Lit::Null });
                if null_test(&re) || null_test(&le) {
                    let subject = if null_test(&re) { le } else { re };
                    return Ok((
                        if op == "==" {
                            Expr::is_null(subject)
                        } else {
                            Expr::is_not_null(subject)
                        },
                        KType::Boolean,
                    ));
                }
                cmp(if op == "==" { BinOp::Eq } else { BinOp::NotEq })
            }
            "<" => cmp(BinOp::Lt),
            "<=" => cmp(BinOp::LtEq),
            ">" => cmp(BinOp::Gt),
            ">=" => cmp(BinOp::GtEq),
            "&&" => Ok((
                Expr::and(Expr::two_valued(le), Expr::two_valued(re)),
                KType::Boolean,
            )),
            "||" => Ok((
                Expr::or(Expr::two_valued(le), Expr::two_valued(re)),
                KType::Boolean,
            )),
            other => gap(format!("operator `{other}`")),
        }
    }
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

fn date_unit(u: &str) -> Option<Unit> {
    Some(match u {
        "yyyy" | "YYYY" => Unit::Year,
        "MM" => Unit::Month,
        "dd" | "DD" => Unit::Day,
        "HH" | "hh" => Unit::Hour,
        "mm" => Unit::Minute,
        "ss" => Unit::Second,
        "SSS" => Unit::Millisecond,
        _ => return None,
    })
}

/// A date pattern of numeric fields only: no month or day names, no week numbers.
fn plain_pattern(p: &str) -> bool {
    !p.is_empty() && p.chars().all(|c| "yMdHhms-/:. T'S".contains(c)) && !p.contains("MMM")
}

fn context_literal(v: &str, ty: KType) -> Expr {
    match ty {
        KType::Integer => v
            .trim()
            .parse::<i64>()
            .map_or_else(|_| Expr::str(v), Expr::int),
        KType::Number | KType::BigNumber => Expr::num(v.trim()),
        KType::Boolean => Expr::bool(v.trim().eq_ignore_ascii_case("true")),
        // A date context's default (`yyyy-MM-dd HH:mm:ss`) is a timestamp, not text: date
        // arithmetic over a text literal does not type-check in SQL.
        KType::Date | KType::Timestamp if is_datetime_text(v) => Expr::Cast {
            arg: Box::new(Expr::str(v.trim())),
            to: KType::Timestamp,
            length: -1,
            precision: -1,
        },
        _ => Expr::str(v),
    }
}

/// `yyyy-MM-dd` with an optional ` HH:mm[:ss]`.
fn is_datetime_text(v: &str) -> bool {
    let v = v.trim();
    let b = v.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| r.clone().all(|i| b.get(i).is_some_and(u8::is_ascii_digit));
    let date = b.len() >= 10
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10);
    let time = match b.len() {
        10 => true,
        16 | 19 => {
            b[10] == b' '
                && digits(11..13)
                && b[13] == b':'
                && digits(14..16)
                && (b.len() == 16 || (b[16] == b':' && digits(17..19)))
        }
        _ => false,
    };
    date && time
}

/// Every `Numeric.sequence(<name>, <start>, <step>)` call in a Java expression, once per call (each
/// call takes the next number): the name, and the start and step when both are integer literals.
pub fn sequences(src: &str) -> Vec<(String, Option<(i64, i64)>)> {
    fn walk(e: &JExpr, out: &mut Vec<(String, Option<(i64, i64)>)>) {
        match e {
            JExpr::Call(Some(recv), name, args) => {
                if name == "sequence"
                    && matches!(recv.as_ref(), JExpr::Name(n) if n == "Numeric")
                    && let Some(JExpr::Str(seq)) = args.first()
                {
                    let lit = |a: Option<&JExpr>| match a {
                        Some(JExpr::Int(i)) => Some(*i),
                        _ => None,
                    };
                    let nums = lit(args.get(1)).zip(lit(args.get(2)));
                    out.push((seq.clone(), nums));
                }
                walk(recv, out);
                for a in args {
                    walk(a, out);
                }
            }
            JExpr::Call(None, _, args) | JExpr::New(_, args) => {
                for a in args {
                    walk(a, out);
                }
            }
            JExpr::Field(t, _) | JExpr::Unary(_, t) | JExpr::Cast(_, t) => walk(t, out),
            JExpr::Binary(_, l, r) | JExpr::Index(l, r) | JExpr::Assign(_, l, r) => {
                walk(l, out);
                walk(r, out);
            }
            JExpr::Ternary(c, a, b) => {
                walk(c, out);
                walk(a, out);
                walk(b, out);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Ok(e) = parse_expr(src) {
        walk(&e, &mut out);
    }
    out
}

/// Whether a lowering result is fully faithful.
pub fn is_exact(l: &Lowered) -> bool {
    l.fidelity == Fidelity::Exact
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope_lower(src: &str) -> Result<(Lowered, KType), Gap> {
        let column = |q: &str, c: &str| match (q, c) {
            ("row1", "QTY") => Some((Expr::col("QTY"), KType::Integer)),
            ("row1", "PRICE") => Some((Expr::col("PRICE"), KType::Number)),
            ("row1", "NAME") => Some((Expr::col("NAME"), KType::String)),
            ("row1", "D") => Some((Expr::col("D"), KType::Timestamp)),
            _ => None,
        };
        let var = |v: &str| (v == "total").then(|| "row1.QTY * row1.PRICE".to_string());
        let context = |v: &str| (v == "schema").then(|| (Some("ODS".to_string()), KType::String));
        lower(
            src,
            &Scope {
                column: &column,
                var: &var,
                context: &context,
                job: "jLoad",
                global: &|_| None,
                routines: &[],
                locale: None,
                sequence: &|_| None,
            },
        )
    }

    fn sql(src: &str) -> String {
        scope_lower(src).unwrap().0.expr.sql()
    }

    #[test]
    fn columns_variables_and_arithmetic_lower_exactly() {
        let (l, t) = scope_lower("row1.QTY * row1.PRICE").unwrap();
        assert_eq!(t, KType::Number);
        assert!(is_exact(&l), "{:?}", l.notes);
        assert_eq!(sql("Var.total"), sql("row1.QTY * row1.PRICE"));
    }

    #[test]
    fn javas_rules_are_named_where_sql_differs() {
        let (l, _) = scope_lower("row1.NAME.equals(\"A\")").unwrap();
        assert!(l.notes.iter().any(|n| n == NOTE_EQUALS_NULL));
        assert!(
            l.expr.sql().contains("IS NOT DISTINCT FROM"),
            "{}",
            l.expr.sql()
        );
        let (l, _) = scope_lower("row1.NAME + \"|\" + row1.QTY").unwrap();
        assert!(l.notes.iter().any(|n| n == NOTE_NULL_CONCAT));
        assert!(l.expr.sql().contains("'null'"), "{}", l.expr.sql());
        let (l, _) = scope_lower("TalendDate.getPartOfDate(\"MONTH\", row1.D)").unwrap();
        assert!(l.notes.iter().any(|n| n == NOTE_MONTH_ZERO));
        let (l, _) = scope_lower("context.schema").unwrap();
        assert_eq!(l.expr.sql(), "'ODS'");
        assert!(l.notes.iter().any(|n| n == NOTE_CONTEXT));
    }

    #[test]
    fn the_last_day_of_the_month_keeps_the_time_and_compare_date_prints_both() {
        let last = sql("TalendDate.getLastDayOfMonth(row1.D)");
        assert!(last.contains("date_trunc('month'"), "{last}");
        let cmp = sql(
            "TalendDate.compareDate(TalendDate.getLastDayOfMonth(row1.D), row1.D, \"yyyy-MM-dd\")==0?1:0",
        );
        assert!(
            cmp.contains("format_datetime") && cmp.contains("IS NULL"),
            "{cmp}"
        );
        assert!(scope_lower("TalendDate.compareDate(row1.D, row1.D, \"MMMM\")").is_err());
    }

    #[test]
    fn a_null_test_is_is_null() {
        assert!(sql("row1.NAME == null ? 0 : 1").contains("IS NULL"));
    }

    #[test]
    fn what_is_not_in_the_catalogue_is_a_gap_with_its_text() {
        assert!(scope_lower("Numeric.sequence(\"s1\", 1, 1)").is_err());
        assert!(
            scope_lower("MyRoutine.clean(row1.NAME)")
                .unwrap_err()
                .reason
                .contains("MyRoutine.clean")
        );
        assert!(scope_lower("row2.X").is_err());
        assert!(
            scope_lower("globalMap.get(\"k\")")
                .unwrap_err()
                .reason
                .contains("globalMap.get(\"k\")")
        );
        assert_eq!(sql("jobName"), "'jLoad'");
        assert!(sql("row1.NAME.matches(\"[A-Z]+\")").contains("^(?:[A-Z]+)$"));
        assert!(scope_lower("row1.NAME.matches(row1.NAME)").is_err());
    }
}

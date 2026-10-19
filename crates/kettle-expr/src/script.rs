//! Symbolic execution of a code step's per-row script (RFC 0203): every variable ends as the SQL
//! expression it holds, `if`/`else` merges into `CASE`, and dropping the row becomes a predicate.

use crate::expr::{BinOp, Expr, Func};
use crate::java::{JExpr, JType, Lowerer};
use crate::semantics::{Gap, Lowered};
use crate::stmt::Stmt;
use crate::types::KType;
use std::collections::BTreeMap;

/// Which language's semantics apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// Modified Java Script Value (Rhino).
    Js,
    /// User Defined Java Class, or a decompiled plugin.
    Java,
}

/// What a script computes for one row.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Every variable's final expression and type (JavaScript outputs are variables).
    pub vars: BTreeMap<String, (Expr, JType)>,
    /// Fields set with `get(Fields.Out, "f").setValue(r, v)` (Java).
    pub outputs: BTreeMap<String, (Expr, JType)>,
    /// When the row is dropped; `None` when it never is.
    pub drop_when: Option<Expr>,
    /// Label and notes of everything lowered.
    pub lowered: Lowered,
}

#[derive(Debug, Clone)]
struct State {
    vars: BTreeMap<String, (Expr, JType)>,
    outputs: BTreeMap<String, (Expr, JType)>,
    /// Whether this path is still running (false after `return`).
    alive: Expr,
    /// When the row has been dropped (JS) or emitted (Java), on this path.
    dropped: Expr,
    emitted: Expr,
    /// How many times this path emits (Java `putRow`).
    emits: u32,
    /// The variable holding the row (Java `Object[] r = getRow()`).
    row_var: Option<String>,
}

/// The executor.
pub struct Executor<'a> {
    pub lang: Lang,
    /// The type of an input field, `None` when there is no such field.
    pub field_type: &'a dyn Fn(&str) -> Option<KType>,
    /// The step's own parameters (`getParameter("P")` in a UDJC).
    pub params: BTreeMap<String, String>,
    out: Lowered,
}

fn is_true(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit {
            value: crate::expr::Lit::Bool(true)
        }
    )
}

fn is_false(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Lit {
            value: crate::expr::Lit::Bool(false)
        }
    )
}

fn and(a: Expr, b: Expr) -> Expr {
    if is_true(&a) {
        b
    } else if is_true(&b) {
        a
    } else if is_false(&a) || is_false(&b) {
        Expr::bool(false)
    } else {
        Expr::and(a, b)
    }
}

fn or(a: Expr, b: Expr) -> Expr {
    if is_false(&a) {
        b
    } else if is_false(&b) {
        a
    } else if is_true(&a) || is_true(&b) {
        Expr::bool(true)
    } else {
        Expr::or(a, b)
    }
}

/// `NOT e`, folding a double negation.
fn not(e: Expr) -> Expr {
    match e {
        Expr::Unary {
            op: crate::expr::UnOp::Not,
            arg,
        } => *arg,
        Expr::Lit {
            value: crate::expr::Lit::Bool(b),
        } => Expr::bool(!b),
        Expr::IsNull { arg, negated } => Expr::IsNull {
            arg,
            negated: !negated,
        },
        e => Expr::negate(e),
    }
}

fn pick(c: &Expr, a: Expr, b: Expr) -> Expr {
    // `c` is two-valued (conditions are), so a boolean choice is the condition itself.
    if is_true(&a) && is_false(&b) {
        return c.clone();
    }
    if is_false(&a) && is_true(&b) {
        return not(c.clone());
    }
    if a == b || is_true(c) {
        a
    } else if is_false(c) {
        b
    } else {
        Expr::case(vec![(c.clone(), a)], Some(b))
    }
}

/// A `get(Fields.In, "f")` read, kept apart from a local that has the field's name.
const FIELD: &str = "\u{1}field:";

const LOGGING: &[&str] = &[
    "writeToLog",
    "logBasic",
    "logDetailed",
    "logDebug",
    "logMinimal",
    "logRowlevel",
    "logError",
    "print",
    "println",
];

impl<'a> Executor<'a> {
    pub fn new(lang: Lang, field_type: &'a dyn Fn(&str) -> Option<KType>) -> Executor<'a> {
        Executor {
            lang,
            field_type,
            params: BTreeMap::new(),
            out: Lowered::exact(Expr::null()),
        }
    }

    /// Runs `stmts` over one row. For Java, a path that never calls `putRow` drops the row.
    pub fn run(mut self, stmts: &[Stmt]) -> Result<Outcome, Gap> {
        let mut st = State {
            vars: BTreeMap::new(),
            outputs: BTreeMap::new(),
            alive: Expr::bool(true),
            dropped: Expr::bool(false),
            emitted: Expr::bool(false),
            emits: 0,
            row_var: None,
        };
        self.block(&mut st, stmts)?;
        let drop_when = match self.lang {
            Lang::Js => (!is_false(&st.dropped)).then_some(st.dropped),
            Lang::Java => {
                if is_true(&st.emitted) {
                    None
                } else if is_false(&st.emitted) {
                    return Err(Gap::new(
                        "processRow never calls putRow: the step writes no rows",
                    ));
                } else {
                    Some(not(st.emitted))
                }
            }
        };
        Ok(Outcome {
            vars: st.vars,
            outputs: st.outputs,
            drop_when,
            lowered: self.out,
        })
    }

    fn block(&mut self, st: &mut State, stmts: &[Stmt]) -> Result<(), Gap> {
        for s in stmts {
            if is_false(&st.alive) {
                break;
            }
            self.stmt(st, s)?;
        }
        Ok(())
    }

    /// Lowers an expression in the current state.
    fn lower(&mut self, st: &State, e: &JExpr) -> Result<(Expr, JType), Gap> {
        let e = self.rewrite(st, e)?;
        let vars = st.vars.clone();
        let ft = self.field_type;
        let nt = |n: &str| -> Option<JType> {
            if let Some(f) = n.strip_prefix(FIELD) {
                return ft(f).map(JType::of_field);
            }
            vars.get(n)
                .map(|(_, t)| *t)
                .or_else(|| ft(n).map(JType::of_field))
        };
        let vars2 = st.vars.clone();
        let ne = |n: &str| -> Expr {
            if let Some(f) = n.strip_prefix(FIELD) {
                return Expr::col(f);
            }
            vars2
                .get(n)
                .map(|(e, _)| e.clone())
                .unwrap_or_else(|| Expr::col(n))
        };
        let mut l = Lowerer::over_fields(&nt, &ne);
        l.js = self.lang == Lang::Js;
        let r = l.lower(&e)?;
        self.out.absorb(&l.out);
        Ok(r)
    }

    /// Kettle's UDJC idioms, before lowering: typed field reads and step parameters.
    fn rewrite(&mut self, st: &State, e: &JExpr) -> Result<JExpr, Gap> {
        if self.lang != Lang::Java {
            return Ok(e.clone());
        }
        Ok(match e {
            JExpr::Call(Some(target), getter, args)
                if getter.starts_with("get") && field_get(target).is_some() =>
            {
                let (dir, field) = field_get(target).expect("checked");
                if dir != "In" && dir != "Out" {
                    return Err(Gap::new(format!("get(Fields.{dir}, …)")));
                }
                let row_ok =
                    matches!(args.as_slice(), [JExpr::Name(r)] if Some(r) == st.row_var.as_ref());
                if !row_ok {
                    return Err(Gap::new(format!(
                        "reading `{field}` from something other than the current row"
                    )));
                }
                let want = match getter.as_str() {
                    "getString" => KType::String,
                    "getInteger" => KType::Integer,
                    "getNumber" => KType::Number,
                    "getBigNumber" => KType::BigNumber,
                    "getDate" => KType::Date,
                    "getBoolean" => KType::Boolean,
                    other => return Err(Gap::new(format!("field accessor {other}"))),
                };
                let have = (self.field_type)(&field)
                    .or_else(|| st.outputs.get(&field).map(|(_, t)| t.ktype()));
                match have {
                    None => return Err(Gap::new(format!("`{field}` is not an input field"))),
                    Some(t) if t != want && t != KType::Unknown => {
                        self.out
                            .note(format!("`{field}` is {t:?}, read with {getter}: converted"));
                    }
                    _ => {}
                }
                JExpr::Name(format!("{FIELD}{field}"))
            }
            JExpr::Call(None, name, args) if name == "getParameter" => match args.as_slice() {
                [JExpr::Str(p)] => match self.params.get(p) {
                    Some(v) => JExpr::Str(v.clone()),
                    None => return Err(Gap::new(format!("step parameter `{p}` is not defined"))),
                },
                _ => return Err(Gap::new("getParameter with a computed name")),
            },
            JExpr::Call(t, n, args) => JExpr::Call(
                t.as_ref()
                    .map(|t| self.rewrite(st, t).map(Box::new))
                    .transpose()?,
                n.clone(),
                args.iter()
                    .map(|a| self.rewrite(st, a))
                    .collect::<Result<_, _>>()?,
            ),
            JExpr::Binary(op, l, r) => JExpr::Binary(
                op,
                Box::new(self.rewrite(st, l)?),
                Box::new(self.rewrite(st, r)?),
            ),
            JExpr::Unary(op, a) => JExpr::Unary(op, Box::new(self.rewrite(st, a)?)),
            JExpr::Ternary(c, a, b) => JExpr::Ternary(
                Box::new(self.rewrite(st, c)?),
                Box::new(self.rewrite(st, a)?),
                Box::new(self.rewrite(st, b)?),
            ),
            JExpr::Cast(t, a) => JExpr::Cast(t.clone(), Box::new(self.rewrite(st, a)?)),
            JExpr::Field(t, n) => JExpr::Field(Box::new(self.rewrite(st, t)?), n.clone()),
            other => other.clone(),
        })
    }

    /// A condition as SQL: JavaScript truthiness, or a Java boolean.
    fn condition(&mut self, st: &State, c: &JExpr) -> Result<Expr, Gap> {
        let (e, t) = self.lower(st, c)?;
        Ok(match (self.lang, t) {
            (_, JType::Bool { .. }) => Expr::two_valued(e),
            (Lang::Js, JType::Long { .. } | JType::Double { .. }) => {
                self.out
                    .note("JavaScript truthiness: a number is true unless 0 or null");
                Expr::two_valued(Expr::bin(BinOp::NotEq, e, Expr::int(0)))
            }
            (Lang::Js, JType::Str) => {
                self.out
                    .note("JavaScript truthiness: a string is true unless empty or null");
                Expr::two_valued(Expr::bin(
                    BinOp::Gt,
                    Expr::call(Func::Length, vec![e]),
                    Expr::int(0),
                ))
            }
            (Lang::Js, JType::Null) => Expr::bool(false),
            (_, other) => return Err(Gap::new(format!("a condition of type {other:?}"))),
        })
    }

    fn assign(&mut self, st: &mut State, name: &str, value: (Expr, JType)) {
        let old = st
            .vars
            .get(name)
            .cloned()
            .or_else(|| (self.field_type)(name).map(|t| (Expr::col(name), JType::of_field(t))))
            .unwrap_or((Expr::null(), value.1));
        let e = pick(&st.alive, value.0, old.0);
        st.vars.insert(name.to_string(), (e, value.1));
    }

    fn stmt(&mut self, st: &mut State, s: &Stmt) -> Result<(), Gap> {
        match s {
            Stmt::Block(b) => self.block(st, b),
            Stmt::Unsupported(kw) => Err(Gap::new(format!("`{kw}` in the step's code"))),
            Stmt::Return(_) => {
                if self.lang == Lang::Js {
                    return Err(Gap::new("`return` in a transform script"));
                }
                st.alive = Expr::bool(false);
                Ok(())
            }
            Stmt::Decl(ds) => {
                for (name, init) in ds {
                    match init {
                        // Java: `Object[] r = getRow();`
                        Some(JExpr::Call(None, f, a))
                            if self.lang == Lang::Java && f == "getRow" && a.is_empty() =>
                        {
                            st.row_var = Some(name.clone());
                        }
                        Some(e) => {
                            let v = self.lower(st, e)?;
                            self.assign(st, name, v);
                        }
                        None => {
                            st.vars
                                .entry(name.clone())
                                .or_insert((Expr::null(), JType::Unknown));
                        }
                    }
                }
                Ok(())
            }
            Stmt::If(c, then, otherwise) => {
                // Java's `if (r == null) { setOutputDone(); return false; }` guard.
                if self.lang == Lang::Java && is_row_null_check(c, st.row_var.as_deref()) {
                    return Ok(());
                }
                // The same guard inverted, as javac lays it out: `if (r != null) { …row… }
                // setOutputDone(); return false;` — the row's path is the `then` block.
                if self.lang == Lang::Java && is_row_present_check(c, st.row_var.as_deref()) {
                    self.block(st, then)?;
                    st.alive = Expr::bool(false);
                    return Ok(());
                }
                let cond = self.condition(st, c)?;
                // Each branch runs relative to its own entry: its assignments are guarded only
                // by returns inside it; the merge guards them by the condition and by whether
                // this path is still running.
                let fresh = |st: &State| State {
                    alive: Expr::bool(true),
                    dropped: Expr::bool(false),
                    emitted: Expr::bool(false),
                    emits: 0,
                    ..st.clone()
                };
                let mut a = fresh(st);
                self.block(&mut a, then)?;
                let mut b = fresh(st);
                self.block(&mut b, otherwise)?;
                let parent = st.alive.clone();
                let mut names: Vec<String> = a.vars.keys().chain(b.vars.keys()).cloned().collect();
                names.sort();
                names.dedup();
                for n in names {
                    let old = st.vars.get(&n).cloned().or_else(|| {
                        (self.field_type)(&n).map(|t| (Expr::col(&n), JType::of_field(t)))
                    });
                    let null = (Expr::null(), JType::Unknown);
                    let va = a
                        .vars
                        .get(&n)
                        .cloned()
                        .or(old.clone())
                        .unwrap_or(null.clone());
                    let vb = b
                        .vars
                        .get(&n)
                        .cloned()
                        .or(old.clone())
                        .unwrap_or(null.clone());
                    let t = if va.1 == JType::Unknown || va.1 == JType::Null {
                        vb.1
                    } else {
                        va.1
                    };
                    let merged = pick(&cond, va.0, vb.0);
                    let prior = old.unwrap_or(null).0;
                    st.vars.insert(n, (pick(&parent, merged, prior), t));
                }
                let mut outs: Vec<String> =
                    a.outputs.keys().chain(b.outputs.keys()).cloned().collect();
                outs.sort();
                outs.dedup();
                for n in outs {
                    let old = st.outputs.get(&n).cloned().or_else(|| {
                        (self.field_type)(&n).map(|t| (Expr::col(&n), JType::of_field(t)))
                    });
                    let null = (Expr::null(), JType::Unknown);
                    let va = a
                        .outputs
                        .get(&n)
                        .cloned()
                        .or(old.clone())
                        .unwrap_or(null.clone());
                    let vb = b
                        .outputs
                        .get(&n)
                        .cloned()
                        .or(old.clone())
                        .unwrap_or(null.clone());
                    let t = if va.1 == JType::Unknown { vb.1 } else { va.1 };
                    let merged = pick(&cond, va.0, vb.0);
                    let prior = old.unwrap_or(null).0;
                    st.outputs.insert(n, (pick(&parent, merged, prior), t));
                }
                let branch_alive = if is_true(&a.alive) && is_true(&b.alive) {
                    Expr::bool(true)
                } else {
                    or(and(cond.clone(), a.alive), and(not(cond.clone()), b.alive))
                };
                st.alive = and(parent.clone(), branch_alive);
                st.dropped = or(
                    st.dropped.clone(),
                    and(parent.clone(), pick(&cond, a.dropped, b.dropped)),
                );
                st.emitted = or(
                    st.emitted.clone(),
                    and(parent, pick(&cond, a.emitted, b.emitted)),
                );
                st.emits += a.emits.max(b.emits);
                if st.row_var.is_none() {
                    st.row_var = a.row_var.or(b.row_var);
                }
                Ok(())
            }
            Stmt::Expr(e) => self.expr_stmt(st, e),
        }
    }

    fn expr_stmt(&mut self, st: &mut State, e: &JExpr) -> Result<(), Gap> {
        match e {
            JExpr::Assign(op, target, value) => {
                let JExpr::Name(name) = target.as_ref() else {
                    return Err(Gap::new("an assignment to something other than a variable"));
                };
                if self.lang == Lang::Js && name == "trans_Status" {
                    return match value.as_ref() {
                        JExpr::Name(v) if v == "SKIP_TRANSFORMATION" => {
                            st.dropped = or(st.dropped.clone(), st.alive.clone());
                            Ok(())
                        }
                        JExpr::Name(v) if v == "CONTINUE_TRANSFORMATION" => Ok(()),
                        _ => Err(Gap::new(
                            "trans_Status set to stop or fail the transformation",
                        )),
                    };
                }
                // `r = getRow();` (a decompiled body declares its locals first).
                if self.lang == Lang::Java
                    && matches!(value.as_ref(), JExpr::Call(None, f, a) if f == "getRow" && a.is_empty())
                {
                    st.row_var = Some(name.clone());
                    return Ok(());
                }
                if self.lang == Lang::Java && Some(name) == st.row_var.as_ref() {
                    // `r = createOutputRow(r, …)`: the row grows; nothing to translate.
                    return match value.as_ref() {
                        JExpr::Call(None, f, _) if f == "createOutputRow" || f == "resizeArray" => {
                            Ok(())
                        }
                        _ => Err(Gap::new("the row array is replaced")),
                    };
                }
                let rhs = if *op == "=" {
                    value.as_ref().clone()
                } else {
                    let bop = op.trim_end_matches('=');
                    let bop: &'static str = ["+", "-", "*", "/", "%"]
                        .into_iter()
                        .find(|b| *b == bop)
                        .ok_or_else(|| Gap::new(format!("operator {op}")))?;
                    JExpr::Binary(bop, target.clone(), value.clone())
                };
                let v = self.lower(st, &rhs)?;
                self.assign(st, name, v);
                Ok(())
            }
            JExpr::Postfix(op, target) | JExpr::Unary(op @ ("++" | "--"), target) => {
                let bop = if *op == "++" { "+" } else { "-" };
                let rhs = JExpr::Binary(bop, target.clone(), Box::new(JExpr::Int(1)));
                self.expr_stmt(st, &JExpr::Assign("=", target.clone(), Box::new(rhs)))
            }
            JExpr::Call(None, f, args) if LOGGING.contains(&f.as_str()) => {
                let _ = args;
                self.out.note_only_logging();
                Ok(())
            }
            JExpr::Call(Some(t), f, _)
                if LOGGING.contains(&f.as_str())
                    && matches!(t.as_ref(), JExpr::Name(n) if n == "System" || n == "Alert") =>
            {
                self.out.note_only_logging();
                Ok(())
            }
            JExpr::Call(Some(t), f, _)
                if matches!(t.as_ref(), JExpr::Field(o, n) if n == "out" && matches!(o.as_ref(), JExpr::Name(s) if s == "System"))
                    && LOGGING.contains(&f.as_str()) =>
            {
                self.out.note_only_logging();
                Ok(())
            }
            JExpr::Call(None, f, args) if self.lang == Lang::Java && f == "putRow" => {
                let _ = args;
                st.emits += 1;
                if st.emits > 1 {
                    return Err(Gap::new(
                        "putRow is called more than once for one input row (rows are multiplied)",
                    ));
                }
                st.emitted = or(st.emitted.clone(), st.alive.clone());
                Ok(())
            }
            JExpr::Call(None, f, _) if self.lang == Lang::Java && f == "setOutputDone" => Ok(()),
            // `get(Fields.Out, "f").setValue(r, v)`
            JExpr::Call(Some(target), m, args) if self.lang == Lang::Java && m == "setValue" => {
                let Some((dir, field)) = field_get(target) else {
                    return Err(Gap::new(
                        "setValue on something other than get(Fields.Out, …)",
                    ));
                };
                if dir != "Out" {
                    return Err(Gap::new(format!("setValue on Fields.{dir}")));
                }
                let [row, value] = args.as_slice() else {
                    return Err(Gap::new("setValue with other than (row, value)"));
                };
                if !matches!(row, JExpr::Name(r) if Some(r) == st.row_var.as_ref()) {
                    return Err(Gap::new("setValue on a row other than the current one"));
                }
                if st.emits > 0 {
                    self.out.note(format!(
                        "`{field}` is set after putRow; the row leaves without it in Kettle"
                    ));
                }
                let v = self.lower(st, value)?;
                let old = st
                    .outputs
                    .get(&field)
                    .cloned()
                    .or_else(|| {
                        (self.field_type)(&field).map(|t| (Expr::col(&field), JType::of_field(t)))
                    })
                    .unwrap_or((Expr::null(), v.1));
                let e = pick(&st.alive, v.0, old.0);
                st.outputs.insert(field, (e, v.1));
                Ok(())
            }
            JExpr::Call(t, f, _) => Err(Gap::new(match t {
                Some(t) => format!(
                    "call to `{}.{f}(…)` (state or an API the translator cannot follow)",
                    short(t)
                ),
                None => {
                    format!("call to `{f}(…)` (a method or function the translator cannot follow)")
                }
            })),
            other => Err(Gap::new(format!("the statement `{}`", short(other)))),
        }
    }
}

/// `get(Fields.In, "f")` → `("In", "f")`.
fn field_get(e: &JExpr) -> Option<(String, String)> {
    let JExpr::Call(None, g, args) = e else {
        return None;
    };
    if g != "get" {
        return None;
    }
    match args.as_slice() {
        [JExpr::Field(f, dir), JExpr::Str(name)] if matches!(f.as_ref(), JExpr::Name(n) if n == "Fields") => {
            Some((dir.clone(), name.clone()))
        }
        _ => None,
    }
}

fn is_row_present_check(c: &JExpr, row: Option<&str>) -> bool {
    matches!(c, JExpr::Binary("!=", l, r) if matches!((l.as_ref(), r.as_ref()), (JExpr::Name(n), JExpr::Null) if Some(n.as_str()) == row))
}

fn is_row_null_check(c: &JExpr, row: Option<&str>) -> bool {
    matches!(c, JExpr::Binary("==", l, r) if matches!((l.as_ref(), r.as_ref()), (JExpr::Name(n), JExpr::Null) if Some(n.as_str()) == row))
}

fn short(e: &JExpr) -> String {
    let s = match e {
        JExpr::Name(n) => n.clone(),
        JExpr::Field(t, n) => format!("{}.{n}", short(t)),
        JExpr::Call(t, n, _) => match t {
            Some(t) => format!("{}.{n}()", short(t)),
            None => format!("{n}()"),
        },
        other => format!("{other:?}"),
    };
    s.chars().take(80).collect()
}

impl Lowered {
    fn note_only_logging(&mut self) {
        let n = "logging calls are dropped".to_string();
        if !self.notes.contains(&n) {
            self.notes.push(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stmt::{parse_class_body, parse_script};

    fn ft(n: &str) -> Option<KType> {
        match n {
            "first" | "last" | "status" | "name" => Some(KType::String),
            "amount" => Some(KType::Number),
            "qty" => Some(KType::Integer),
            _ => None,
        }
    }

    #[test]
    fn javascript_branches_merge_and_skip_becomes_a_predicate() {
        let s = parse_script(
            "var full = trim(first) + ' ' + trim(last);\nvar tier = 'low';\nif (amount > 1000) { tier = 'high'; } else if (amount > 100) tier = 'mid';\nif (status == 'void') trans_Status = SKIP_TRANSFORMATION;\nvar half = qty / 2;",
            true,
        )
        .unwrap();
        let o = Executor::new(Lang::Js, &ft).run(&s).unwrap();
        assert_eq!(
            o.vars["tier"].0.sql(),
            "CASE WHEN coalesce((coalesce(\"amount\", 0) > 1000), FALSE) THEN 'high' ELSE CASE WHEN coalesce((coalesce(\"amount\", 0) > 100), FALSE) THEN 'mid' ELSE 'low' END END"
        );
        assert_eq!(
            o.drop_when.unwrap().sql(),
            "(\"status\" IS NOT DISTINCT FROM 'void')"
        );
        assert_eq!(
            o.vars["half"].0.sql(),
            "(CAST(coalesce(\"qty\", 0) AS double) / 2)"
        );
        assert!(
            o.vars["full"]
                .0
                .sql()
                .contains("coalesce(trim(\"first\"), 'null')")
        );
    }

    #[test]
    fn a_udjc_process_row_becomes_outputs_and_a_filter() {
        let src = r#"public boolean processRow(StepMetaInterface smi, StepDataInterface sdi) throws KettleException {
            Object[] r = getRow();
            if (r == null) { setOutputDone(); return false; }
            r = createOutputRow(r, data.outputRowMeta.size());
            String name = get(Fields.In, "name").getString(r);
            Long qty = get(Fields.In, "qty").getInteger(r);
            if (qty == null) { return true; }
            String label = name.toUpperCase() + "-" + getParameter("SUFFIX");
            get(Fields.Out, "label").setValue(r, label);
            logBasic("row");
            putRow(data.outputRowMeta, r);
            return true;
        }"#;
        let b = parse_class_body(src).unwrap();
        let mut ex = Executor::new(Lang::Java, &ft);
        ex.params.insert("SUFFIX".into(), "X".into());
        let o = ex.run(&b.methods[0].1).unwrap();
        assert_eq!(o.drop_when.unwrap().sql(), "(\"qty\" IS NULL)");
        assert!(
            o.outputs["label"].0.sql().contains("upper(\"name\")"),
            "{}",
            o.outputs["label"].0.sql()
        );
        assert!(o.lowered.notes.iter().any(|n| n.contains("logging")));
    }

    /// RFC 0205 bench, run 1: Rhino reads a null number as 0 in arithmetic, so
    /// `parseInt('42') + qty` with `qty` null is 42; Java has no such rule.
    #[test]
    fn a_null_number_is_zero_in_javascript_arithmetic_only() {
        let s = parse_script("var total = parseInt('42') + qty;", true).unwrap();
        let o = Executor::new(Lang::Js, &ft).run(&s).unwrap();
        let sql = o.vars["total"].0.sql();
        assert!(sql.contains("coalesce(\"qty\", 0)"), "{sql}");
        assert!(
            o.lowered
                .notes
                .iter()
                .any(|n| n.contains("null number as 0"))
        );

        let src = r#"public boolean processRow(StepMetaInterface smi, StepDataInterface sdi) throws KettleException {
            Object[] r = getRow();
            if (r == null) { setOutputDone(); return false; }
            r = createOutputRow(r, data.outputRowMeta.size());
            Long qty = get(Fields.In, "qty").getInteger(r);
            get(Fields.Out, "total").setValue(r, qty + 1);
            putRow(data.outputRowMeta, r);
            return true;
        }"#;
        let b = parse_class_body(src).unwrap();
        let o = Executor::new(Lang::Java, &ft).run(&b.methods[0].1).unwrap();
        let sql = o.outputs["total"].0.sql();
        assert!(!sql.contains("coalesce(\"qty\", 0)"), "{sql}");
    }

    #[test]
    fn what_the_executor_cannot_follow_is_named() {
        let s = parse_script("for (var i = 0; i < 3; i++) {}", true).unwrap();
        assert!(
            Executor::new(Lang::Js, &ft)
                .run(&s)
                .unwrap_err()
                .reason
                .contains("`for`")
        );
        let s = parse_script("var x = getInputRowMeta().size();", true).unwrap();
        assert!(Executor::new(Lang::Js, &ft).run(&s).is_err());
        let src = "public boolean processRow(StepMetaInterface smi, StepDataInterface sdi) { Object[] r = getRow(); counter++; putRow(data.outputRowMeta, r); putRow(data.outputRowMeta, r); return true; }";
        let b = parse_class_body(src).unwrap();
        assert!(Executor::new(Lang::Java, &ft).run(&b.methods[0].1).is_err());
    }
}

#[cfg(test)]
mod decompiled {
    use super::*;
    use crate::stmt::parse_class_body;

    /// RFC 0206: `processRow` exactly as the private JVM decompiler renders a compiled plugin
    /// (golden fixture `AcmeDoubler`): locals declared first, the row guard inverted, autoboxing
    /// spelled out.
    #[test]
    fn a_decompiled_plugin_process_row_executes() {
        let src = "public boolean processRow(StepMetaInterface smi, StepDataInterface sdi) {\n\
            Object[] r;\nLong qty;\nString status;\nr = getRow();\nif ((r != null)) {\n\
            r = createOutputRow(r, 4);\nqty = get(Fields.In, \"qty\").getInteger(r);\n\
            status = get(Fields.In, \"status\").getString(r);\nif ((qty != null)) {\n\
            get(Fields.Out, \"doubled\").setValue(r, Long.valueOf((qty.longValue() * 2)));\n\
            get(Fields.Out, \"label\").setValue(r, ((\"\" + status.toUpperCase()) + \"#\") + qty);\n\
            putRow(null, r);\nreturn true;\n}\nreturn true;\n}\nsetOutputDone();\nreturn false;\n}";
        let ft = |n: &str| match n {
            "qty" => Some(KType::Integer),
            "status" => Some(KType::String),
            _ => None,
        };
        let b = parse_class_body(src).unwrap();
        let o = Executor::new(Lang::Java, &ft).run(&b.methods[0].1).unwrap();
        assert_eq!(
            o.outputs["doubled"].0.sql(),
            "CASE WHEN (\"qty\" IS NOT NULL) THEN (\"qty\" * 2) ELSE NULL END"
        );
        let label = o.outputs["label"].0.sql();
        assert!(
            label.contains("concat('', coalesce(upper(\"status\"), 'null'))"),
            "{label}"
        );
        assert_eq!(o.drop_when.unwrap().sql(), "(\"qty\" IS NULL)");
    }
}

//! Fidelity labels and the Kettle rules RFC 0202 makes explicit.

use crate::expr::{BinOp, Expr, Func};
use serde::{Deserialize, Serialize};

/// How faithfully a step (or an expression) was carried over (RFC 0200 §2). Ordered from the
/// strongest to the weakest, so `max` is the weakest of several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fidelity {
    Exact,
    Rewritten,
    LlmDrafted,
    Gap,
}

impl Fidelity {
    pub fn as_str(self) -> &'static str {
        match self {
            Fidelity::Exact => "exact",
            Fidelity::Rewritten => "rewritten",
            Fidelity::LlmDrafted => "llm-drafted",
            Fidelity::Gap => "gap",
        }
    }
}

/// Why something could not be translated: the text that stopped it, in words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub reason: String,
}

impl Gap {
    pub fn new(reason: impl Into<String>) -> Gap {
        Gap {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for Gap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

/// A translated expression with the rules applied to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lowered {
    pub expr: Expr,
    pub fidelity: Fidelity,
    /// Every rule below `exact`, in words a reviewer can check.
    pub notes: Vec<String>,
}

impl Lowered {
    pub fn exact(expr: Expr) -> Lowered {
        Lowered {
            expr,
            fidelity: Fidelity::Exact,
            notes: Vec::new(),
        }
    }

    pub fn rewritten(expr: Expr, note: impl Into<String>) -> Lowered {
        Lowered {
            expr,
            fidelity: Fidelity::Rewritten,
            notes: vec![note.into()],
        }
    }

    /// Records `note` and lowers the label to at least `rewritten`.
    pub fn note(&mut self, note: impl Into<String>) {
        let note = note.into();
        if !self.notes.contains(&note) {
            self.notes.push(note);
        }
        self.fidelity = self.fidelity.max(Fidelity::Rewritten);
    }

    /// Takes over `other`'s label and notes (for an expression built from sub-expressions).
    pub fn absorb(&mut self, other: &Lowered) {
        self.fidelity = self.fidelity.max(other.fidelity);
        for n in &other.notes {
            if !self.notes.contains(n) {
                self.notes.push(n.clone());
            }
        }
    }
}

pub const NOTE_NULL_COMPARE: &str = "Kettle compares nulls as values (two nulls are equal, null sorts first): written with \
     IS [NOT] DISTINCT FROM and explicit null branches";
pub const NOTE_CONCAT_NULLS: &str =
    "Kettle concatenation skips null operands: written as concat over coalesce(x, '')";
pub const NOTE_REGEX_FULL: &str =
    "Kettle regular expressions must match the whole value (Java matches()): anchored with ^(?:…)$";

/// Kettle's comparison `l <op> r` as a two-valued SQL predicate. `l_null`/`r_null`: whether
/// each side can be null (a non-null literal cannot).
pub fn kettle_compare(op: BinOp, l: Expr, r: Expr, l_null: bool, r_null: bool) -> Expr {
    if !l_null && !r_null {
        return Expr::bin(op, l, r);
    }
    match op {
        BinOp::Eq => Expr::bin(BinOp::NotDistinct, l, r),
        BinOp::NotEq => Expr::bin(BinOp::Distinct, l, r),
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            // Null is the smallest value; two nulls are equal.
            let (when_l_null, when_r_null) = match op {
                BinOp::Lt => (Expr::is_not_null(r.clone()), Expr::bool(false)),
                BinOp::LtEq => (Expr::bool(true), Expr::bool(false)),
                BinOp::Gt => (Expr::bool(false), Expr::is_not_null(l.clone())),
                _ => (Expr::is_null(r.clone()), Expr::bool(true)),
            };
            let mut when = Vec::new();
            if l_null {
                when.push((Expr::is_null(l.clone()), when_l_null));
            }
            if r_null {
                when.push((Expr::is_null(r.clone()), when_r_null));
            }
            Expr::case(when, Some(Expr::bin(op, l, r)))
        }
        other => Expr::bin(other, l, r),
    }
}

/// Kettle's string concatenation: nulls skipped, an empty result is null.
pub fn kettle_concat(parts: Vec<Expr>) -> Expr {
    let coalesced = parts
        .into_iter()
        .map(|p| Expr::call(Func::Coalesce, vec![p, Expr::str("")]))
        .collect();
    Expr::call(
        Func::NullIf,
        vec![Expr::call(Func::Concat, coalesced), Expr::str("")],
    )
}

/// `^(?:pattern)$`: Java `matches()` semantics for Trino's `regexp_like`.
pub fn full_match(pattern: &str) -> String {
    format!("^(?:{pattern})$")
}

/// Whether `e` can be null: a non-null literal cannot; anything else may.
pub fn nullable(e: &Expr) -> bool {
    !matches!(
        e,
        Expr::Lit {
            value: crate::expr::Lit::Str(_)
                | crate::expr::Lit::Int(_)
                | crate::expr::Lit::Num(_)
                | crate::expr::Lit::Bool(_)
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparisons_follow_kettle_null_rules() {
        let c = |op| kettle_compare(op, Expr::col("a"), Expr::int(5), true, false).sql();
        assert_eq!(c(BinOp::Eq), "(\"a\" IS NOT DISTINCT FROM 5)");
        assert_eq!(
            c(BinOp::Lt),
            "CASE WHEN (\"a\" IS NULL) THEN (5 IS NOT NULL) ELSE (\"a\" < 5) END"
        );
        assert_eq!(
            c(BinOp::Gt),
            "CASE WHEN (\"a\" IS NULL) THEN FALSE ELSE (\"a\" > 5) END"
        );
        let both = kettle_compare(BinOp::GtEq, Expr::col("a"), Expr::col("b"), true, true).sql();
        assert_eq!(
            both,
            "CASE WHEN (\"a\" IS NULL) THEN (\"b\" IS NULL) WHEN (\"b\" IS NULL) THEN TRUE ELSE (\"a\" >= \"b\") END"
        );
        assert_eq!(
            kettle_compare(BinOp::Eq, Expr::int(1), Expr::int(2), false, false).sql(),
            "(1 = 2)"
        );
    }

    #[test]
    fn concatenation_skips_nulls() {
        assert_eq!(
            kettle_concat(vec![Expr::col("a"), Expr::col("b")]).sql(),
            "nullif(concat(coalesce(\"a\", ''), coalesce(\"b\", '')), '')"
        );
        assert!(Fidelity::Gap > Fidelity::Rewritten);
    }
}

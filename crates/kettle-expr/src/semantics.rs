//! Kettle's rules where they differ from SQL (RFC 0202). `Fidelity`, `Gap` and `Lowered` are
//! engine-neutral and live in `ekos-lakehouse-etl-expr` (RFC 0266); they are re-exported here.

pub use ekos_lakehouse_etl_expr::semantics::*;

use crate::expr::{BinOp, Expr, Func};

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

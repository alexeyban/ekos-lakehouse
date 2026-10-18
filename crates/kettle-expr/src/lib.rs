//! Typed Kettle expressions (RFC 0202): one expression tree with a closed function set, a
//! parser per Kettle expression language (condition trees, Calculator, Formula, Java), the
//! Kettle rules that differ from SQL made explicit, and Trino rendering. Pure.

pub mod calculator;
pub mod condition;
pub mod expr;
pub mod formula;
pub mod java;
pub mod semantics;
pub mod types;

pub use expr::{BinOp, Expr, Func, Lit, UnOp, Unit, infer, quote_ident, quote_str};
pub use semantics::{Fidelity, Gap, Lowered};
pub use types::KType;

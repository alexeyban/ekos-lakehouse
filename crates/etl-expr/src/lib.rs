//! Engine-neutral typed expressions (RFC 0266, extracted from RFC 0202's `kettle-expr`): one
//! expression tree with a closed function set and Trino rendering, the fidelity label and the gap
//! every lowering reports, and a Java lexer, expression parser and statement parser. Pure.
//!
//! What each ETL tool's expressions mean stays with that tool: Kettle's lowering (empty string
//! is null, null-safe comparison) is in `kettle-expr`; Talend's is RFC 0267's.

pub mod expr;
pub mod java;
pub mod semantics;
pub mod stmt;
pub mod types;

pub use expr::{BinOp, Expr, Func, Lit, UnOp, Unit, infer, quote_ident, quote_str};
pub use semantics::{Fidelity, Gap, Lowered};
pub use types::KType;

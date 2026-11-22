//! Talend jobs as the engine-neutral relational IR (EKOS RFC 0267). Pure: an expanded
//! `ekos_talend::Job` and a catalogue in, an `etl_rel::Ir` out, with every place the SQL differs
//! from the Java named and everything it cannot translate a gap with its text.
//!
//! - [`expr`]: Talend's Java expressions over `etl-expr`'s parser, with Talend's rules.
//! - [`lower`]: components as nodes.

pub mod expr;
pub mod lower;
pub mod routine;
pub mod sql;

pub use lower::{ContextFromData, Lowering, lower_job};

//! The bronze layer of EKOS Lakehouse (RFC 0192): what each source table lands as, how it is
//! loaded, and the SQL that does it. Pure functions over source facts — no connection, no clock,
//! no randomness — so the same facts always generate the same files.

pub mod engine;
pub mod extraction;
pub mod mode;
pub mod model;
pub mod plan;
pub mod render;
pub mod typemap;
pub mod typemap_kit;
pub mod typemap_oracle;

pub use engine::{EngineSpec, OracleSpec, PostgresSpec};
pub use mode::{Cadence, DeletePlan, LoadMode, ModeSettings, PurgePolicy};
pub use model::{
    CodeFacts, ColumnCheck, ForeignKey, SourceColumn, SourceEngine, SourceTable, TypeKind,
    UpdateTrigger, ValueList, WriteFacts,
};
pub use plan::{
    ColumnRename, GoldEngine, Plan, TablePlan, attach_checks, attach_code, carry_renames,
    iceberg_name, plan, plan_for, plan_with, safe_name,
};
pub use typemap::{CompareRule, Lossiness, Mapping};

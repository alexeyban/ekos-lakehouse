//! The relational IR of a Kettle transformation (RFC 0202): one typed node per step (one per
//! target for a routing step), each with its output schema and how faithfully it was carried
//! over. Pure: the tables a Table input reads are typed through a [`Catalog`] the caller
//! supplies.

mod build;
pub mod model;
pub mod source;
mod steps;

pub use build::{build, build_with};
pub use ekos_lakehouse_etl_rel::model::*;
pub use steps::{convert, declared_outputs};

pub use ekos_lakehouse_etl_rel::{Catalog, NoCatalog};

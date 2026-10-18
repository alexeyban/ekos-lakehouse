//! The relational IR of a Kettle transformation (RFC 0202): one typed node per step (one per
//! target for a routing step), each with its output schema and how faithfully it was carried
//! over. Pure: the tables a Table input reads are typed through a [`Catalog`] the caller
//! supplies.

mod build;
pub mod model;
pub mod source;
mod steps;

pub use build::{build, build_with};
pub use model::*;
pub use steps::{convert, declared_outputs};

/// Column types of the tables a transformation reads and writes. The translator's catalog is
/// the lakehouse lock file (silver models); tests use a fixed one.
pub trait Catalog {
    /// The columns of `table`, or `None` when it is not known.
    fn columns(&self, table: &TableRef) -> Option<Vec<Field>>;
}

/// A catalog that knows no table.
pub struct NoCatalog;

impl Catalog for NoCatalog {
    fn columns(&self, _table: &TableRef) -> Option<Vec<Field>> {
        None
    }
}

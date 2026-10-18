//! What a transformation's tables are in the lakehouse: a silver model of a lakehouse source,
//! the gold model another transformation of the package writes, or unknown (RFC 0204).

use ekos_lakehouse_bronze::plan::{Plan, TablePlan};
use ekos_lakehouse_kettle::Connection;
use ekos_lakehouse_kettle_expr::KType;
use ekos_lakehouse_kettle_rel::{Catalog, Field, TableRef};
use std::collections::BTreeMap;

/// A table as the package names it: (database, schema, table), lower-cased.
pub type TableKey = (String, String, String);

/// What a table resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// A silver model of the plan: its dbt name and fields.
    Silver {
        model: String,
        fields: Vec<Field>,
    },
    /// A gold model this package writes.
    Gold {
        model: String,
        fields: Vec<Field>,
    },
    Unknown(String),
}

/// The Kettle type of an Iceberg column type in Trino's spelling.
pub fn ktype_of_iceberg(t: &str) -> KType {
    let t = t.trim().to_ascii_lowercase();
    if t.starts_with("varchar") || t.starts_with("char") || t == "uuid" || t == "json" {
        KType::String
    } else if matches!(t.as_str(), "bigint" | "integer" | "smallint" | "tinyint") {
        KType::Integer
    } else if matches!(t.as_str(), "double" | "real") {
        KType::Number
    } else if t.starts_with("decimal") {
        KType::BigNumber
    } else if t.starts_with("timestamp") {
        KType::Timestamp
    } else if t == "date" {
        KType::Date
    } else if t == "boolean" {
        KType::Boolean
    } else if t == "varbinary" {
        KType::Binary
    } else {
        KType::Unknown
    }
}

/// Resolves tables against the deployed plan and the package's own outputs.
#[derive(Debug, Clone)]
pub struct Resolver<'a> {
    pub plan: &'a Plan,
    /// The lakehouse source database (`[extensions.lakehouse] source-database`).
    pub source_database: Option<String>,
    /// Tables the package writes: key → (gold model, fields).
    pub written: BTreeMap<TableKey, (String, Vec<Field>)>,
}

/// Whether a Kettle database type is PostgreSQL.
pub fn is_postgres(db_type: &str) -> bool {
    matches!(
        db_type.to_ascii_uppercase().as_str(),
        "POSTGRESQL" | "GREENPLUM"
    )
}

/// The key a table is known by in the package.
pub fn key(t: &TableRef) -> TableKey {
    let db = t
        .connection
        .as_ref()
        .and_then(|c| c.database.clone())
        .unwrap_or_default()
        .to_lowercase();
    let schema = t
        .schema
        .clone()
        .unwrap_or_else(|| "public".into())
        .to_lowercase();
    (db, schema, t.table.to_lowercase())
}

impl Resolver<'_> {
    /// Whether `c` reads the lakehouse source.
    pub fn is_source(&self, c: Option<&Connection>) -> bool {
        let Some(c) = c else { return false };
        if !is_postgres(&c.db_type) {
            return false;
        }
        match (&self.source_database, &c.database) {
            (Some(want), Some(db)) => want.eq_ignore_ascii_case(db),
            (Some(_), None) => false,
            (None, _) => true,
        }
    }

    fn plan_table(&self, t: &TableRef) -> Option<&TablePlan> {
        let schema = t.schema.clone().unwrap_or_else(|| "public".into());
        self.plan.tables.iter().find(|p| {
            p.schema.eq_ignore_ascii_case(&schema) && p.table.eq_ignore_ascii_case(&t.table)
        })
    }

    pub fn resolve(&self, t: &TableRef) -> Resolution {
        if let Some((model, fields)) = self.written.get(&key(t)) {
            return Resolution::Gold {
                model: model.clone(),
                fields: fields.clone(),
            };
        }
        let conn = t.connection.as_ref();
        if !self.is_source(conn) {
            let what = conn
                .map(|c| {
                    format!(
                        "connection `{}` ({} {})",
                        c.name,
                        c.db_type,
                        c.database.as_deref().unwrap_or("?")
                    )
                })
                .unwrap_or_else(|| "no connection".into());
            return Resolution::Unknown(format!(
                "`{}` is read through {what}, which is not the lakehouse source",
                t.table
            ));
        }
        match self.plan_table(t) {
            Some(p) => Resolution::Silver {
                model: ekos_lakehouse_silver::model_name(p),
                fields: p
                    .columns
                    .iter()
                    .filter(|m| !m.iceberg_type.is_empty())
                    .map(|m| Field::new(&m.column, ktype_of_iceberg(&m.iceberg_type)))
                    .collect(),
            },
            None => Resolution::Unknown(format!(
                "`{}.{}` is not a table of the lakehouse plan (refused, or not in the configured schemas)",
                t.schema.as_deref().unwrap_or("public"),
                t.table
            )),
        }
    }
}

impl Catalog for Resolver<'_> {
    fn columns(&self, table: &TableRef) -> Option<Vec<Field>> {
        match self.resolve(table) {
            Resolution::Silver { fields, .. } | Resolution::Gold { fields, .. } => Some(fields),
            Resolution::Unknown(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iceberg_types_map_back() {
        assert_eq!(ktype_of_iceberg("decimal(12,2)"), KType::BigNumber);
        assert_eq!(
            ktype_of_iceberg("timestamp(6) with time zone"),
            KType::Timestamp
        );
        assert_eq!(ktype_of_iceberg("varchar"), KType::String);
        assert_eq!(ktype_of_iceberg("array(varchar)"), KType::Unknown);
    }
}

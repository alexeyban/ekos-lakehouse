//! Table input: the step's SQL parsed in its connection's dialect, for the tables it reads and
//! the fields it returns.

use crate::Catalog;
use crate::model::{Field, TableRef};
use ekos_lakehouse_kettle::Connection;
use ekos_lakehouse_kettle_expr::{Gap, KType};
use sqlparser::ast::{
    Expr as SqlExpr, Ident, ObjectName, Query, SelectItem, SetExpr, Statement, TableFactor,
    visit_relations,
};
use sqlparser::dialect::{
    Dialect, GenericDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect, SnowflakeDialect,
};
use std::ops::ControlFlow;

/// The sqlparser dialect for a Kettle database type.
pub fn dialect(db_type: &str) -> Box<dyn Dialect> {
    match db_type.to_ascii_uppercase().as_str() {
        "POSTGRESQL" | "GREENPLUM" | "REDSHIFT" | "VERTICA" | "VERTICA5" => {
            Box::new(PostgreSqlDialect {})
        }
        "MSSQL" | "MSSQLNATIVE" | "SYBASE" => Box::new(MsSqlDialect {}),
        "MYSQL" | "MARIADB" => Box::new(MySqlDialect {}),
        "SNOWFLAKE" => Box::new(SnowflakeDialect {}),
        _ => Box::new(GenericDialect {}),
    }
}

/// Whether unquoted identifiers fold to lower case on this database (PostgreSQL family).
fn folds_lower(db_type: &str) -> bool {
    matches!(
        db_type.to_ascii_uppercase().as_str(),
        "POSTGRESQL" | "GREENPLUM" | "REDSHIFT" | "VERTICA" | "VERTICA5"
    )
}

/// What a Table input returns.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub tables: Vec<TableRef>,
    pub fields: Vec<Field>,
    /// Fields whose type could not be derived (an expression, an unknown table).
    pub untyped: Vec<String>,
}

fn ident_name(i: &Ident, lower: bool) -> String {
    if i.quote_style.is_none() && lower {
        i.value.to_lowercase()
    } else {
        i.value.clone()
    }
}

fn table_ref(name: &ObjectName, connection: Option<&Connection>, lower: bool) -> TableRef {
    let parts: Vec<String> = name.0.iter().map(|i| ident_name(i, lower)).collect();
    let (schema, table) = match parts.as_slice() {
        [t] => (None, t.clone()),
        [.., s, t] => (Some(s.clone()), t.clone()),
        [] => (None, String::new()),
    };
    TableRef {
        connection: connection.cloned(),
        schema: schema.or_else(|| connection.and_then(|c| c.schema.clone())),
        table,
    }
}

/// Parses a Table input's SQL.
pub fn parse(
    sql: &str,
    connection: Option<&Connection>,
    catalog: &dyn Catalog,
) -> Result<Parsed, Gap> {
    let db_type = connection.map(|c| c.db_type.as_str()).unwrap_or("");
    let d = dialect(db_type);
    let lower = folds_lower(db_type);
    let stmts = sqlparser::parser::Parser::parse_sql(d.as_ref(), sql)
        .map_err(|e| Gap::new(format!("the SQL does not parse as {db_type}: {e}")))?;
    let [stmt] = stmts.as_slice() else {
        return Err(Gap::new(format!(
            "{} statements; a Table input runs one query",
            stmts.len()
        )));
    };
    let Statement::Query(q) = stmt else {
        return Err(Gap::new("the SQL is not a query"));
    };
    let ctes: Vec<String> = q
        .with
        .as_ref()
        .map(|w| {
            w.cte_tables
                .iter()
                .map(|c| ident_name(&c.alias.name, lower))
                .collect()
        })
        .unwrap_or_default();
    let mut tables = Vec::new();
    let _ = visit_relations(stmt, |r| {
        let t = table_ref(r, connection, lower);
        if !ctes.contains(&t.table) && !tables.contains(&t) {
            tables.push(t);
        }
        ControlFlow::<()>::Continue(())
    });
    let (fields, untyped) = fields_of(q, connection, catalog, lower)?;
    Ok(Parsed {
        tables,
        fields,
        untyped,
    })
}

fn fields_of(
    q: &Query,
    connection: Option<&Connection>,
    catalog: &dyn Catalog,
    lower: bool,
) -> Result<(Vec<Field>, Vec<String>), Gap> {
    let select = match q.body.as_ref() {
        SetExpr::Select(s) => s,
        SetExpr::SetOperation { left, .. } => match left.as_ref() {
            SetExpr::Select(s) => s,
            _ => {
                return Err(Gap::new(
                    "a set operation whose first branch is not a SELECT",
                ));
            }
        },
        SetExpr::Query(inner) => return fields_of(inner, connection, catalog, lower),
        _ => return Err(Gap::new("the query body is not a SELECT")),
    };
    // (alias or table name, columns) per FROM item that is a table.
    let mut scopes: Vec<(String, Option<Vec<Field>>)> = Vec::new();
    let mut add = |f: &TableFactor| {
        if let TableFactor::Table { name, alias, .. } = f {
            let t = table_ref(name, connection, lower);
            let key = alias
                .as_ref()
                .map(|a| ident_name(&a.name, lower))
                .unwrap_or(t.table.clone());
            scopes.push((key, catalog.columns(&t)));
        } else if let TableFactor::Derived { alias, .. } = f {
            let key = alias
                .as_ref()
                .map(|a| ident_name(&a.name, lower))
                .unwrap_or_default();
            scopes.push((key, None));
        }
    };
    for twj in &select.from {
        add(&twj.relation);
        for j in &twj.joins {
            add(&j.relation);
        }
    }
    let lookup = |qualifier: Option<&str>, col: &str| -> Option<Field> {
        scopes
            .iter()
            .filter(|(k, _)| qualifier.is_none_or(|q| q.eq_ignore_ascii_case(k)))
            .filter_map(|(_, cols)| cols.as_ref())
            .flat_map(|cols| cols.iter())
            .find(|f| f.name.eq_ignore_ascii_case(col))
            .cloned()
    };
    let mut fields = Vec::new();
    let mut untyped = Vec::new();
    let mut push = |name: String, typed: Option<Field>| match typed {
        Some(mut f) => {
            f.name = name;
            fields.push(f);
        }
        None => {
            untyped.push(name.clone());
            fields.push(Field::new(&name, KType::Unknown));
        }
    };
    for item in &select.projection {
        match item {
            SelectItem::Wildcard(_) => {
                for (k, cols) in &scopes {
                    let Some(cols) = cols else {
                        return Err(Gap::new(format!(
                            "`*` over `{k}`, whose columns are not known"
                        )));
                    };
                    for c in cols {
                        push(c.name.clone(), Some(c.clone()));
                    }
                }
            }
            SelectItem::QualifiedWildcard(name, _) => {
                let q = name
                    .0
                    .last()
                    .map(|i| ident_name(i, lower))
                    .unwrap_or_default();
                let Some((_, Some(cols))) = scopes.iter().find(|(k, _)| k.eq_ignore_ascii_case(&q))
                else {
                    return Err(Gap::new(format!(
                        "`{q}.*` over a relation whose columns are not known"
                    )));
                };
                for c in cols.clone() {
                    push(c.name.clone(), Some(c));
                }
            }
            SelectItem::UnnamedExpr(e) => match e {
                SqlExpr::Identifier(i) => {
                    let n = ident_name(i, lower);
                    push(n.clone(), lookup(None, &n));
                }
                SqlExpr::CompoundIdentifier(parts) => {
                    let n = parts
                        .last()
                        .map(|i| ident_name(i, lower))
                        .unwrap_or_default();
                    let q = (parts.len() > 1).then(|| ident_name(&parts[parts.len() - 2], lower));
                    push(n.clone(), lookup(q.as_deref(), &n));
                }
                other => {
                    return Err(Gap::new(format!(
                        "the column `{other}` has no name (give it an alias)"
                    )));
                }
            },
            SelectItem::ExprWithAlias { expr, alias } => {
                let n = ident_name(alias, lower);
                let typed = match expr {
                    SqlExpr::Identifier(i) => lookup(None, &ident_name(i, lower)),
                    SqlExpr::CompoundIdentifier(parts) => {
                        let c = parts
                            .last()
                            .map(|i| ident_name(i, lower))
                            .unwrap_or_default();
                        let q =
                            (parts.len() > 1).then(|| ident_name(&parts[parts.len() - 2], lower));
                        lookup(q.as_deref(), &c)
                    }
                    _ => None,
                };
                push(n, typed);
            }
        }
    }
    Ok((fields, untyped))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Shop;
    impl Catalog for Shop {
        fn columns(&self, t: &TableRef) -> Option<Vec<Field>> {
            match t.table.as_str() {
                "orders" => Some(vec![
                    Field::new("id", KType::Integer),
                    Field::new("customer_id", KType::Integer),
                    Field::new("total", KType::BigNumber),
                ]),
                "customers" => Some(vec![
                    Field::new("id", KType::Integer),
                    Field::new("name", KType::String),
                ]),
                _ => None,
            }
        }
    }

    fn pg() -> Connection {
        Connection {
            name: "src".into(),
            db_type: "POSTGRESQL".into(),
            schema: Some("sales".into()),
            ..Default::default()
        }
    }

    #[test]
    fn types_come_from_the_catalog_and_tables_are_listed() {
        let p = parse(
            "SELECT o.ID, c.name AS customer, o.total * 2 AS doubled FROM Orders o JOIN customers c ON c.id = o.customer_id",
            Some(&pg()),
            &Shop,
        )
        .unwrap();
        assert_eq!(
            p.tables
                .iter()
                .map(|t| t.table.as_str())
                .collect::<Vec<_>>(),
            vec!["orders", "customers"]
        );
        assert_eq!(
            p.tables[0].schema.as_deref(),
            Some("sales"),
            "the connection's preferred schema"
        );
        let f: Vec<_> = p.fields.iter().map(|f| (f.name.as_str(), f.ty)).collect();
        assert_eq!(
            f,
            vec![
                ("id", KType::Integer),
                ("customer", KType::String),
                ("doubled", KType::Unknown)
            ]
        );
        assert_eq!(p.untyped, vec!["doubled".to_string()]);
    }

    #[test]
    fn star_needs_known_columns_and_bad_sql_is_a_gap() {
        let p = parse("select * from orders", Some(&pg()), &Shop).unwrap();
        assert_eq!(p.fields.len(), 3);
        assert!(parse("select * from unknown_t", Some(&pg()), &Shop).is_err());
        assert!(parse("selec x", Some(&pg()), &Shop).is_err());
        assert!(
            parse("select count(*) from orders", Some(&pg()), &Shop).is_err(),
            "unnamed column"
        );
        let mssql = Connection {
            db_type: "MSSQLNATIVE".into(),
            ..Default::default()
        };
        let p = parse(
            "SELECT TOP 10 [OrderID] FROM [Sales].[Orders]",
            Some(&mssql),
            &Shop,
        )
        .unwrap();
        assert_eq!(p.tables[0].schema.as_deref(), Some("Sales"));
        assert_eq!(p.fields[0].name, "OrderID");
    }
}

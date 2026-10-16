//! Source facts for the bronze plan (RFC 0192 §1): PostgreSQL's own catalog, read through EKOS's
//! read-only session (RFC 0157), and the Trino connector's column list for the same tables.

use ekos_lakehouse_bronze::{SourceColumn, SourceTable, TypeKind, UpdateTrigger};
use ekos_lakehouse_trino::TrinoReader;
use ekos_pg_live::catalog::CatalogSource;
use std::collections::BTreeMap;

fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn in_list(schemas: &[String]) -> String {
    schemas
        .iter()
        .map(|s| lit(s))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Base tables and partitioned parents of `schemas`, with columns, keys, update triggers and the
/// connector's view of each column.
pub fn read(
    pg: &dyn CatalogSource,
    trino: &TrinoReader,
    trino_catalog: &str,
    schemas: &[String],
) -> anyhow::Result<Vec<SourceTable>> {
    let list = in_list(schemas);
    let mut tables: BTreeMap<(String, String), SourceTable> = BTreeMap::new();
    for r in pg.rows(&format!(
        "SELECT n.nspname, c.relname, GREATEST(c.reltuples, 0)::bigint \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname IN ({list}) AND c.relkind IN ('r', 'p') AND NOT c.relispartition \
         ORDER BY 1, 2"
    ))? {
        tables.insert(
            (r[0].clone(), r[1].clone()),
            SourceTable {
                schema: r[0].clone(),
                name: r[1].clone(),
                columns: vec![],
                primary_key: vec![],
                row_estimate: r[2].parse().unwrap_or(0),
                update_triggers: vec![],
            },
        );
    }

    for r in pg.rows(&format!(
        "SELECT n.nspname, c.relname, a.attname, format_type(a.atttypid, a.atttypmod), \
                t.typtype, t.typcategory, \
                CASE WHEN t.typtype = 'd' THEN format_type(t.typbasetype, t.typtypmod) END, \
                CASE WHEN t.typcategory = 'A' THEN format_type(t.typelem, NULL) END, \
                a.attnotnull \
         FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_type t ON t.oid = a.atttypid \
         WHERE n.nspname IN ({list}) AND c.relkind IN ('r', 'p') AND NOT c.relispartition \
           AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY 1, 2, a.attnum"
    ))? {
        let kind = match (r[4].as_str(), r[5].as_str()) {
            (_, "A") => TypeKind::Array {
                element: r[7].clone(),
            },
            ("e", _) => TypeKind::Enum,
            ("d", _) => TypeKind::Domain { base: r[6].clone() },
            ("b", _) => TypeKind::Base,
            _ => TypeKind::Other,
        };
        if let Some(t) = tables.get_mut(&(r[0].clone(), r[1].clone())) {
            t.columns.push(SourceColumn {
                name: r[2].clone(),
                pg_type: r[3].clone(),
                kind,
                not_null: r[8] == "t",
                trino_type: None,
                numeric_used: None,
            });
        }
    }

    for r in pg.rows(&format!(
        "SELECT n.nspname, c.relname, a.attname \
         FROM pg_index i \
         JOIN pg_class c ON c.oid = i.indrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = ANY (i.indkey) \
         WHERE i.indisprimary AND n.nspname IN ({list}) \
         ORDER BY 1, 2, array_position(i.indkey::int2[], a.attnum)"
    ))? {
        if let Some(t) = tables.get_mut(&(r[0].clone(), r[1].clone())) {
            t.primary_key.push(r[2].clone());
        }
    }

    // BEFORE (2) UPDATE (16) FOR EACH ROW (1) triggers, with their function's source.
    for r in pg.rows(&format!(
        "SELECT n.nspname, c.relname, tg.tgname, p.oid::regprocedure::text, p.prosrc \
         FROM pg_trigger tg \
         JOIN pg_class c ON c.oid = tg.tgrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_proc p ON p.oid = tg.tgfoid \
         WHERE NOT tg.tgisinternal AND tg.tgenabled <> 'D' \
           AND (tg.tgtype & 1) = 1 AND (tg.tgtype & 2) = 2 AND (tg.tgtype & 16) = 16 \
           AND n.nspname IN ({list}) \
         ORDER BY 1, 2, 3"
    ))? {
        if let Some(t) = tables.get_mut(&(r[0].clone(), r[1].clone())) {
            let assigns = assigned_new_columns(&r[4]);
            if !assigns.is_empty() {
                t.update_triggers.push(UpdateTrigger {
                    trigger: r[2].clone(),
                    function: r[3].clone(),
                    assigns,
                });
            }
        }
    }

    // What the connector exposes.
    let rows = trino
        .read(&format!(
            "SELECT table_schema, table_name, column_name, data_type \
             FROM \"{}\".information_schema.columns WHERE table_schema IN ({list})",
            trino_catalog.replace('"', "\"\"")
        ))
        .map_err(|e| anyhow::anyhow!("reading the connector's columns: {e}"))?;
    let mut exposed: BTreeMap<(String, String, String), String> = BTreeMap::new();
    for r in rows {
        exposed.insert((r[0].clone(), r[1].clone(), r[2].clone()), r[3].clone());
    }
    for t in tables.values_mut() {
        for c in &mut t.columns {
            c.trino_type = exposed
                .get(&(t.schema.clone(), t.name.clone(), c.name.clone()))
                .cloned();
        }
    }

    // Unconstrained numerics: measure the precision and scale used (RFC 0192 §2).
    for t in tables.values_mut() {
        let qualified = format!("{}.{}", ident(&t.schema), ident(&t.name));
        for c in &mut t.columns {
            if c.pg_type == "numeric" {
                c.numeric_used = measure_numeric(pg, &qualified, &c.name)?;
            }
        }
    }
    Ok(tables.into_values().collect())
}

fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// `(precision, scale)` the data uses, or `None` when a special value (NaN, ±Infinity) has no
/// decimal landing. A full scan: only bronze-sized tables reach this.
fn measure_numeric(
    pg: &dyn CatalogSource,
    table: &str,
    column: &str,
) -> anyhow::Result<Option<(u32, u32)>> {
    let col = ident(column);
    let r = pg.rows(&format!(
        "SELECT COALESCE(max(scale({col})), 0), \
                COALESCE(max(length(trunc(abs({col}))::text)), 1), \
                count(*) FILTER (WHERE {col} IN ('NaN', 'Infinity', '-Infinity')) \
         FROM {table} WHERE {col} IS NOT NULL"
    ))?;
    let row = &r[0];
    let (scale, int_digits, specials): (u32, u32, u64) = (
        row[0].parse().unwrap_or(0),
        row[1].parse().unwrap_or(1),
        row[2].parse().unwrap_or(1),
    );
    Ok((specials == 0).then_some((int_digits + scale, scale)))
}

/// Columns a PL/pgSQL body assigns as `NEW.<col> := …` or `NEW.<col> = …` at the start of a
/// statement. A comparison (`IF NEW.x = OLD.x`) is not an assignment: what precedes it decides.
pub fn assigned_new_columns(body: &str) -> Vec<String> {
    let lower = body.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(pos) = lower[i..].find("new.") {
        let at = i + pos;
        i = at + 4;
        if at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_') {
            continue;
        }
        // What precedes: start, `;`, or a block keyword.
        let before = lower[..at].trim_end();
        let starts_statement = before.is_empty()
            || before.ends_with(';')
            || ["begin", "then", "else", "loop"].iter().any(|k| {
                before.ends_with(k)
                    && !before[..before.len() - k.len()]
                        .ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            });
        if !starts_statement {
            continue;
        }
        let rest = &lower[at + 4..];
        let name_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if name_len == 0 {
            continue;
        }
        let after = rest[name_len..].trim_start();
        if after.starts_with(":=") || (after.starts_with('=') && !after.starts_with("=>")) {
            let name = rest[..name_len].to_string();
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagila_last_updated_assigns_last_update() {
        let body = "\nBEGIN\n    NEW.last_update = CURRENT_TIMESTAMP;\n    RETURN NEW;\nEND ";
        assert_eq!(assigned_new_columns(body), ["last_update"]);
    }

    #[test]
    fn comparisons_and_references_are_not_assignments() {
        let body = "BEGIN IF NEW.a = OLD.a THEN NEW.b := now(); END IF; \
                    PERFORM f(NEW.c); x := NEW.d; RETURN NEW; END";
        assert_eq!(assigned_new_columns(body), ["b"]);
        assert!(assigned_new_columns("tsvector_update_trigger").is_empty());
    }
}

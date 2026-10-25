//! RFC 0216 §2.3: an Oracle source, read through `ekos-ora-live`'s guarded session (RFC 0211):
//! pinned NLS settings, time zone UTC, `DBA_*` views when the session may read them, `ALL_*`
//! otherwise, and every catalog read inside one read-only transaction.
//!
//! Names keep Oracle's case (`ADEMPIERE.C_ORDER`): reconciliation reads the source directly and
//! needs them as Oracle spells them, and Trino folds every identifier, quoted or not, so the
//! generated SQL reads the same tables through the connector.

use crate::source::{Source, assigned_new_columns};
use ekos_lakehouse_bronze::typemap_oracle::NO_VALUES_SCALE;
use ekos_lakehouse_bronze::{
    ForeignKey, SourceColumn, SourceEngine, SourceTable, TypeKind, UpdateTrigger,
};
use ekos_lakehouse_trino::TrinoReader;
use ekos_migrate_validate::{Dialect, EngineReader, ReadError};
use ekos_ora_live::{OraConnection, OraSource, SessionPolicy, ViewFamily};
use std::collections::BTreeMap;

/// `STANDARD_HASH` hashes a string's bytes in the database character set: the canonical form is
/// UTF-8, so reconciliation needs an AL32UTF8 database (RFC 0216 §2.5).
const CHARSET: &str = "AL32UTF8";

pub struct Oracle {
    source: OraSource,
    family: ViewFamily,
    /// The database character set, read at connect.
    charset: String,
    label: String,
}

impl Oracle {
    /// Connect as a project's `[[databases]]` entry: its Easy Connect string, user and password
    /// variable (RFC 0211).
    pub fn connect(
        db: &ekos_compiler_core::config::DatabaseConfig,
        purpose: &str,
    ) -> anyhow::Result<Self> {
        let conn = OraConnection {
            connect_string: db.connect.clone(),
            user: db.user.clone(),
            password_env: db.password_env.clone(),
        };
        let source = OraSource::connect(&conn, purpose, &SessionPolicy::default())?;
        let family = source.family();
        let charset = source
            .rows_of(
                "SELECT value FROM nls_database_parameters WHERE parameter = 'NLS_CHARACTERSET'",
                &[],
            )?
            .into_iter()
            .next()
            .and_then(|r| r.into_iter().next().flatten())
            .unwrap_or_default();
        Ok(Self {
            label: source.label().to_string(),
            source,
            family,
            charset,
        })
    }

    fn rows(&self, sql: &str, binds: &[&str]) -> anyhow::Result<Vec<Vec<String>>> {
        Ok(self
            .source
            .rows_of(sql, binds)?
            .into_iter()
            .map(|r| r.into_iter().map(Option::unwrap_or_default).collect())
            .collect())
    }

    fn read_owner(
        &self,
        owner: &str,
        redaction: &ekos_common::redaction::RedactionConfig,
        max_measured_rows: i64,
    ) -> anyhow::Result<Vec<SourceTable>> {
        let f = self.family.prefix();
        let redact = |s: &str| ekos_common::redaction::redact(s, redaction);
        let mut tables: BTreeMap<String, SourceTable> = BTreeMap::new();
        // Tables, not temporary, nested, dropped or a materialized view's container.
        for r in self.rows(
            &format!(
                "SELECT t.table_name, NVL(t.num_rows, -1), o.object_id FROM {f}_tables t \
                 JOIN {f}_objects o ON o.owner = t.owner AND o.object_name = t.table_name \
                   AND o.object_type = 'TABLE' \
                 WHERE t.owner = :1 AND t.temporary = 'N' AND t.nested = 'NO' AND t.dropped = 'NO' \
                   AND t.table_name NOT LIKE 'BIN$%' \
                   AND t.table_name NOT IN (SELECT mview_name FROM {f}_mviews WHERE owner = :2)"
            ),
            &[owner, owner],
        )? {
            tables.insert(
                r[0].clone(),
                SourceTable {
                    schema: owner.to_string(),
                    name: r[0].clone(),
                    oid: r[2].parse().unwrap_or(0),
                    columns: vec![],
                    primary_key: vec![],
                    row_estimate: r[1].parse().unwrap_or(-1),
                    update_triggers: vec![],
                    foreign_keys: vec![],
                    comment: None,
                },
            );
        }

        for r in self.rows(
            &format!(
                "SELECT table_name, column_name, column_id, data_type, data_length, \
                        data_precision, data_scale, char_length, char_used, nullable, \
                        CASE WHEN default_length > 0 THEN 'Y' ELSE 'N' END \
                 FROM {f}_tab_columns WHERE owner = :1 ORDER BY table_name, column_id"
            ),
            &[owner],
        )? {
            let Some(t) = tables.get_mut(&r[0]) else {
                continue;
            };
            let n = |i: usize| r[i].trim().parse::<i64>().ok();
            let data_type = ekos_ora_live::catalog::render_type(
                &r[3],
                n(4),
                n(5),
                n(6),
                n(7),
                (!r[8].is_empty()).then_some(r[8].as_str()),
            );
            t.columns.push(SourceColumn {
                name: r[1].clone(),
                pg_type: data_type,
                kind: TypeKind::Base,
                not_null: r[9] == "N",
                trino_type: None,
                numeric_used: None,
                comment: None,
                enum_labels: vec![],
                pii_candidate: ekos_common::pii::classify_name(&r[1].to_lowercase())
                    .map(|c| format!("{:?}", c.class).to_lowercase()),
                infinite_values: None,
                attnum: r[2].parse().unwrap_or(0),
                has_default: r[10] == "Y",
            });
        }

        for r in self.rows(
            &format!(
                "SELECT c.table_name, cc.column_name FROM {f}_constraints c \
                 JOIN {f}_cons_columns cc ON cc.owner = c.owner \
                   AND cc.constraint_name = c.constraint_name \
                 WHERE c.owner = :1 AND c.constraint_type = 'P' \
                 ORDER BY c.table_name, cc.position"
            ),
            &[owner],
        )? {
            if let Some(t) = tables.get_mut(&r[0]) {
                t.primary_key.push(r[1].clone());
            }
        }

        // Declared foreign keys, column pairs in key order.
        let mut fks: BTreeMap<(String, String), ForeignKey> = BTreeMap::new();
        for r in self.rows(
            &format!(
                "SELECT c.table_name, c.constraint_name, r.owner, r.table_name, \
                        cc.column_name, rc.column_name \
                 FROM {f}_constraints c \
                 JOIN {f}_constraints r ON r.owner = c.r_owner \
                   AND r.constraint_name = c.r_constraint_name \
                 JOIN {f}_cons_columns cc ON cc.owner = c.owner \
                   AND cc.constraint_name = c.constraint_name \
                 JOIN {f}_cons_columns rc ON rc.owner = r.owner \
                   AND rc.constraint_name = r.constraint_name AND rc.position = cc.position \
                 WHERE c.owner = :1 AND c.constraint_type = 'R' \
                 ORDER BY c.table_name, c.constraint_name, cc.position"
            ),
            &[owner],
        )? {
            let fk = fks
                .entry((r[0].clone(), r[1].clone()))
                .or_insert_with(|| ForeignKey {
                    name: r[1].clone(),
                    columns: vec![],
                    ref_schema: r[2].clone(),
                    ref_table: r[3].clone(),
                    ref_columns: vec![],
                });
            fk.columns.push(r[4].clone());
            fk.ref_columns.push(r[5].clone());
        }
        for ((table, _), fk) in fks {
            if let Some(t) = tables.get_mut(&table) {
                t.foreign_keys.push(fk);
            }
        }

        for r in self.rows(
            &format!(
                "SELECT table_name, comments FROM {f}_tab_comments \
                 WHERE owner = :1 AND comments IS NOT NULL"
            ),
            &[owner],
        )? {
            if let Some(t) = tables.get_mut(&r[0]) {
                t.comment = Some(redact(&r[1]));
            }
        }
        for r in self.rows(
            &format!(
                "SELECT table_name, column_name, comments FROM {f}_col_comments \
                 WHERE owner = :1 AND comments IS NOT NULL"
            ),
            &[owner],
        )? {
            if let Some(c) = tables
                .get_mut(&r[0])
                .and_then(|t| t.columns.iter_mut().find(|c| c.name == r[1]))
            {
                c.comment = Some(redact(&r[2]));
            }
        }

        // BEFORE EACH ROW triggers on UPDATE, with their body: a `:NEW.<col> :=` proves a
        // watermark as a PostgreSQL trigger's `NEW.<col> :=` does.
        for r in self.rows(
            &format!(
                "SELECT table_name, trigger_name, trigger_body FROM {f}_triggers \
                 WHERE owner = :1 AND base_object_type = 'TABLE' AND status = 'ENABLED' \
                   AND trigger_type = 'BEFORE EACH ROW' AND triggering_event LIKE '%UPDATE%' \
                 ORDER BY table_name, trigger_name"
            ),
            &[owner],
        )? {
            if let Some(t) = tables.get_mut(&r[0]) {
                // Unquoted PL/SQL identifiers fold to upper case, as the catalog spells them.
                let assigns: Vec<String> = assigned_new_columns(&r[2])
                    .into_iter()
                    .map(|c| c.to_uppercase())
                    .collect();
                if !assigns.is_empty() {
                    use sha2::{Digest, Sha256};
                    t.update_triggers.push(UpdateTrigger {
                        trigger: r[1].clone(),
                        function: r[1].clone(),
                        assigns,
                        body_sha: format!("{:x}", Sha256::digest(r[2].as_bytes())),
                    });
                }
            }
        }

        for t in tables.values_mut() {
            if (0..=max_measured_rows).contains(&t.row_estimate) {
                self.measure_numbers(t)?;
            }
        }
        Ok(tables.into_values().collect())
    }

    /// Integer digits and scale an unconstrained `NUMBER`'s data uses, for every such column of
    /// `t` in one scan. A column with no values (an empty table, or all NULL) is measured as
    /// precision 0 at [`NO_VALUES_SCALE`].
    fn measure_numbers(&self, t: &mut SourceTable) -> anyhow::Result<()> {
        let cols: Vec<usize> = t
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.pg_type == "NUMBER")
            .map(|(i, _)| i)
            .collect();
        if cols.is_empty() {
            return Ok(());
        }
        let mut select = Vec::new();
        for &i in &cols {
            let c = q(&t.columns[i].name);
            select.push(format!(
                "MAX(CASE WHEN {c} = TRUNC({c}) THEN 0 \
                 ELSE LENGTH(TO_CHAR(ABS({c}) - TRUNC(ABS({c})))) - 1 END)"
            ));
            select.push(format!("MAX(LENGTH(TO_CHAR(TRUNC(ABS({c})))))"));
        }
        let rows = self.rows(
            &format!(
                "SELECT {} FROM {}.{}",
                select.join(", "),
                q(&t.schema),
                q(&t.name)
            ),
            &[],
        )?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(());
        };
        for (k, &i) in cols.iter().enumerate() {
            let scale: Option<u32> = row[2 * k].parse().ok();
            let digits: Option<u32> = row[2 * k + 1].parse().ok();
            t.columns[i].numeric_used = Some(match (scale, digits) {
                (Some(s), Some(d)) => (d + s, s),
                _ => (0, NO_VALUES_SCALE),
            });
        }
        Ok(())
    }
}

/// An Oracle identifier, quoted. Catalog names are used as Oracle spells them.
fn q(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

impl Source for Oracle {
    fn engine(&self) -> SourceEngine {
        SourceEngine::Oracle
    }

    fn read_tables(
        &self,
        trino: &TrinoReader,
        trino_catalog: &str,
        schemas: &[String],
        redaction: &ekos_common::redaction::RedactionConfig,
        max_measured_rows: i64,
    ) -> anyhow::Result<Vec<SourceTable>> {
        // One snapshot for every catalog query: a table created between two of them cannot make
        // the lists disagree.
        self.source.begin_read_only()?;
        let read: anyhow::Result<Vec<SourceTable>> = schemas
            .iter()
            .map(|owner| self.read_owner(owner, redaction, max_measured_rows))
            .collect::<anyhow::Result<Vec<_>>>()
            .map(|v| v.into_iter().flatten().collect());
        self.source.end_transaction()?;
        let mut tables = read?;

        // What the connector exposes. Trino folds names to lower case; Oracle's are upper.
        let list = schemas
            .iter()
            .map(|s| format!("'{}'", s.to_lowercase().replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        let rows = trino
            .read(&format!(
                "SELECT table_schema, table_name, column_name, data_type \
                 FROM \"{}\".information_schema.columns WHERE table_schema IN ({list})",
                trino_catalog.replace('"', "\"\"")
            ))
            .map_err(|e| anyhow::anyhow!("reading the connector's columns: {e}"))?;
        let exposed: BTreeMap<(String, String, String), String> = rows
            .into_iter()
            .map(|r| ((r[0].clone(), r[1].clone(), r[2].clone()), r[3].clone()))
            .collect();
        for t in &mut tables {
            let (schema, table) = (t.schema.to_lowercase(), t.name.to_lowercase());
            for c in &mut t.columns {
                c.trino_type = exposed
                    .get(&(schema.clone(), table.clone(), c.name.to_lowercase()))
                    .cloned();
            }
        }
        Ok(tables)
    }

    fn reader(&self) -> &dyn EngineReader {
        self
    }

    fn writes_refused(&self) -> anyhow::Result<bool> {
        // A zero-row insert into a table the user can read, always rolled back (RFC 0211).
        let probe = self.rows(
            "SELECT owner, table_name FROM all_tables WHERE owner <> USER AND ROWNUM = 1",
            &[],
        )?;
        let Some(r) = probe.into_iter().next() else {
            anyhow::bail!("this user can read no other schema's table to probe a write on");
        };
        Ok(self.source.check_write_refused(&r[0], &r[1])?.is_safe())
    }
}

impl EngineReader for Oracle {
    fn dialect(&self) -> Dialect {
        Dialect::Oracle
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, ReadError> {
        if self.charset != CHARSET {
            return Err(ReadError::Query {
                engine: self.label.clone(),
                message: format!(
                    "the database character set is {}, not {CHARSET}: Oracle hashes text in the \
                     database character set, so its row hashes cannot equal the canonical UTF-8 \
                     ones (RFC 0216 §2.5)",
                    self.charset
                ),
            });
        }
        self.rows(sql, &[]).map_err(|e| ReadError::Query {
            engine: self.label.clone(),
            message: e.to_string(),
        })
    }
}

//! RFC 0192 §3: how a table is loaded, chosen from facts and recorded with them.

use crate::model::SourceTable;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum LoadMode {
    /// `CREATE OR REPLACE TABLE … AS SELECT`: the whole table, one atomic commit per run.
    Full,
    /// Rows whose `watermark` is at or after the highest one already loaded; with a key, a key-set
    /// diff appends a tombstone for each key the source no longer has.
    Incremental {
        watermark: String,
        keyset_deletes: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ModeSettings {
    /// `"schema.table" = "column"`: a column the team says moves on every write.
    pub watermarks: BTreeMap<String, String>,
    /// Above this many rows, a table loaded in full is called out in the plan.
    pub full_refresh_max_rows: i64,
}

impl Default for ModeSettings {
    fn default() -> Self {
        Self {
            watermarks: BTreeMap::new(),
            full_refresh_max_rows: 5_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeDecision {
    pub mode: LoadMode,
    /// The facts the decision rests on.
    pub evidence: Vec<String>,
    /// What a person should know: a declared watermark that was not proven, a large full load.
    pub notes: Vec<String>,
}

/// Types a watermark can be compared on.
fn orderable(pg_type: &str) -> bool {
    pg_type.starts_with("timestamp") || matches!(pg_type, "date" | "integer" | "bigint")
}

pub fn choose(t: &SourceTable, s: &ModeSettings) -> ModeDecision {
    let mut d = ModeDecision {
        mode: LoadMode::Full,
        evidence: vec![],
        notes: vec![],
    };
    if let Some(w) = s.watermarks.get(&t.qualified()) {
        match t.column(w) {
            None => d.notes.push(format!(
                "declared watermark {w} is not a column of {}",
                t.qualified()
            )),
            Some(c) if !orderable(&c.pg_type) => d.notes.push(format!(
                "declared watermark {w} is {}, which cannot order writes",
                c.pg_type
            )),
            Some(_) => {
                let proof = t
                    .update_triggers
                    .iter()
                    .find(|tr| tr.assigns.iter().any(|a| a == w));
                match proof {
                    Some(tr) => {
                        d.evidence
                            .push(format!("{w} declared as the watermark in settings"));
                        d.evidence.push(format!(
                            "trigger {} runs {} before every update and assigns NEW.{w}",
                            tr.trigger, tr.function
                        ));
                        if !tr.body_sha.is_empty() {
                            // A changed routine is a change (RFC 0194): the proof names its body.
                            d.evidence.push(format!(
                                "{} body sha256 {}",
                                tr.function,
                                &tr.body_sha[..tr.body_sha.len().min(16)]
                            ));
                        }
                        let keyed = !t.primary_key.is_empty();
                        if keyed {
                            d.evidence.push(format!(
                                "primary key ({}) allows a key-set diff for deletes",
                                t.primary_key.join(", ")
                            ));
                        } else {
                            d.notes.push(
                                "no primary key: deletes in the source are not detected".into(),
                            );
                        }
                        d.mode = LoadMode::Incremental {
                            watermark: w.clone(),
                            keyset_deletes: keyed,
                        };
                        return d;
                    }
                    None => d.notes.push(format!(
                        "{w} is declared, but no BEFORE UPDATE row trigger assigns NEW.{w}: \
                         an update that does not touch it would be missed, so the table is \
                         loaded in full"
                    )),
                }
            }
        }
    }
    d.evidence.push(format!("about {} rows", t.row_estimate));
    if t.row_estimate > s.full_refresh_max_rows {
        d.notes.push(format!(
            "more than {} rows loaded in full on every run: declare and prove a watermark",
            s.full_refresh_max_rows
        ));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SourceColumn, TypeKind, UpdateTrigger};

    fn table(triggers: Vec<UpdateTrigger>, pk: &[&str], rows: i64) -> SourceTable {
        let c = |n: &str, t: &str| SourceColumn {
            name: n.into(),
            pg_type: t.into(),
            kind: TypeKind::Base,
            not_null: true,
            trino_type: None,
            numeric_used: None,
            comment: None,
            enum_labels: vec![],
            pii_candidate: None,
            infinite_values: None,
            attnum: 0,
            has_default: false,
        };
        SourceTable {
            schema: "public".into(),
            oid: 0,
            name: "film".into(),
            columns: vec![
                c("film_id", "integer"),
                c("title", "text"),
                c("last_update", "timestamp with time zone"),
            ],
            primary_key: pk.iter().map(|s| s.to_string()).collect(),
            row_estimate: rows,
            update_triggers: triggers,
            foreign_keys: vec![],
            comment: None,
        }
    }

    fn pagila_trigger() -> UpdateTrigger {
        UpdateTrigger {
            trigger: "last_updated".into(),
            function: "public.last_updated()".into(),
            assigns: vec!["last_update".into()],
            body_sha: String::new(),
        }
    }

    fn declared(col: &str) -> ModeSettings {
        ModeSettings {
            watermarks: [("public.film".to_string(), col.to_string())].into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_declared_and_proven_watermark_is_incremental_with_deletes() {
        let d = choose(
            &table(vec![pagila_trigger()], &["film_id"], 1000),
            &declared("last_update"),
        );
        assert_eq!(
            d.mode,
            LoadMode::Incremental {
                watermark: "last_update".into(),
                keyset_deletes: true
            }
        );
        assert!(
            d.evidence
                .iter()
                .any(|e| e.contains("trigger last_updated")),
            "{d:?}"
        );
    }

    #[test]
    fn declared_but_unproven_is_full_and_says_why() {
        let d = choose(&table(vec![], &["film_id"], 1000), &declared("last_update"));
        assert_eq!(d.mode, LoadMode::Full);
        assert!(d.notes[0].contains("no BEFORE UPDATE row trigger"), "{d:?}");
    }

    #[test]
    fn a_trigger_alone_is_not_a_declaration() {
        let d = choose(
            &table(vec![pagila_trigger()], &["film_id"], 1000),
            &ModeSettings::default(),
        );
        assert_eq!(d.mode, LoadMode::Full);
    }

    #[test]
    fn bad_declarations_are_full_with_a_note() {
        for (col, needle) in [("nope", "not a column"), ("title", "cannot order")] {
            let d = choose(
                &table(vec![pagila_trigger()], &["film_id"], 1),
                &declared(col),
            );
            assert_eq!(d.mode, LoadMode::Full);
            assert!(d.notes[0].contains(needle), "{col}: {d:?}");
        }
    }

    #[test]
    fn no_key_means_no_delete_detection_and_a_note() {
        let d = choose(
            &table(vec![pagila_trigger()], &[], 1000),
            &declared("last_update"),
        );
        assert!(matches!(
            d.mode,
            LoadMode::Incremental {
                keyset_deletes: false,
                ..
            }
        ));
        assert!(d.notes.iter().any(|n| n.contains("deletes")));
    }

    #[test]
    fn a_large_full_load_is_called_out() {
        let d = choose(
            &table(vec![], &["film_id"], 10_000_000),
            &ModeSettings::default(),
        );
        assert_eq!(d.mode, LoadMode::Full);
        assert!(d.notes.iter().any(|n| n.contains("declare and prove")));
    }
}

//! The Lakehouse tab of the EKOS web console (RFC 0199 §1, built on EKOS RFC 0198).
//!
//! The page is data: panels show the JSON of `lakehouse status|tables|proposals --json` (all
//! class read), and actions run catalogue commands as jobs. `tables` and `proposals` read the
//! pipeline repository only — its lock file and its git branches — never the source or Trino.

use crate::plan_cmd::read_lock;
use crate::settings::Settings;
use crate::sync::git;
use ekos::extension::{Action, Column, ConsolePage, PageParam, Panel, View};
use ekos_lakehouse_bronze::mode::LoadMode;
use ekos_lakehouse_bronze::typemap::Lossiness;
use serde_json::{Value, json};
use std::path::Path;

/// The console page (RFC 0198).
pub fn pages() -> Vec<ConsolePage> {
    vec![
        ConsolePage {
            id: "lakehouse",
            title: "Lakehouse",
            description: "PostgreSQL to Iceberg, bronze to gold: the state of every table, how each one \
                      loads, and the schema changes and marts waiting for a merge. Buttons run as \
                      jobs; their output is in the run's log.",
            panels: vec![
                Panel {
                    id: "health",
                    title: "Health per table and layer",
                    command: vec!["lakehouse", "status", "--json"],
                    view: View::Table {
                        rows: "",
                        columns: vec![
                            Column::new("/table", "Table"),
                            Column::new("/layer", "Layer"),
                            Column::badge("/state", "State"),
                            Column::new("/evidence/0", "Why"),
                            Column::new("/cause", "Probable cause"),
                        ],
                    },
                    empty: "No runs recorded yet. Generate the pipeline and run it once \
                        (the Dagster job `bronze`).",
                    ..Default::default()
                },
                Panel {
                    id: "proposals",
                    title: "Waiting for a merge",
                    command: vec!["lakehouse", "proposals", "--json"],
                    view: View::Table {
                        rows: "/proposals",
                        columns: vec![
                            Column::badge("/kind", "Kind"),
                            Column::new("/subject", "Table or mart"),
                            Column::new("/summary", "What changes"),
                            Column::new("/branch", "Branch"),
                            Column::new("/at", "Proposed"),
                        ],
                    },
                    empty: "Nothing waiting: every source change was safe, and no mart is proposed.",
                    ..Default::default()
                },
                Panel {
                    id: "tables",
                    title: "Tables and how they load",
                    command: vec!["lakehouse", "tables", "--json"],
                    view: View::Table {
                        rows: "/tables",
                        columns: vec![
                            Column::new("/table", "Table"),
                            Column::badge("/mode", "Load mode"),
                            Column::new("/watermark", "Watermark"),
                            Column::new("/primary_key", "Key"),
                            Column::new("/not_exact", "Columns not landed exactly"),
                        ],
                    },
                    empty: "No pipeline yet: run `lakehouse plan`, then `lakehouse generate`.",
                    ..Default::default()
                },
                Panel {
                    id: "refused",
                    title: "Tables not loaded",
                    command: vec!["lakehouse", "tables", "--json"],
                    view: View::Table {
                        rows: "/refused",
                        columns: vec![
                            Column::new("/table", "Table"),
                            Column::new("/reasons", "Why"),
                        ],
                    },
                    empty: "Every table of the source is loaded.",
                    ..Default::default()
                },
                Panel {
                    id: "stack",
                    title: "Stack",
                    command: vec!["lakehouse", "tables", "--json"],
                    view: View::Fields {
                        fields: vec![
                            Column::new("/settings/trino", "Trino"),
                            Column::new("/settings/dagster", "Dagster"),
                            Column::new("/settings/polaris", "Polaris health"),
                            Column::new("/settings/storage", "Storage"),
                            Column::new("/settings/source", "Source"),
                            Column::new("/settings/target_catalog", "Iceberg catalog"),
                            Column::new("/settings/bronze_schedule", "Bronze schedule"),
                            Column::new("/settings/pipeline", "Pipeline repository"),
                        ],
                    },
                    empty: "No `[extensions.lakehouse]` settings in this project's ekos.toml.",
                    ..Default::default()
                },
            ],
            actions: vec![
                Action {
                    label: "Doctor",
                    command: vec!["lakehouse", "doctor"],
                    ..Default::default()
                },
                Action {
                    label: "Sync source changes",
                    command: vec!["lakehouse", "sync"],
                    ..Default::default()
                },
                Action {
                    label: "Check the pipeline",
                    command: vec!["lakehouse", "check"],
                    ..Default::default()
                },
                Action {
                    label: "Plan",
                    command: vec!["lakehouse", "plan"],
                    ..Default::default()
                },
                Action {
                    label: "Generate",
                    command: vec!["lakehouse", "generate"],
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        pentaho_page(),
    ]
}

/// RFC 0207: the Pentaho tab — packages, then one translation's graph, models, steps, a step's
/// original text next to its SQL, questions and the proposal; buttons to scan and translate.
/// No approve or merge: merging the proposal branch is the approval (RFC 0190).
fn pentaho_page() -> ConsolePage {
    let report = || vec!["lakehouse", "pentaho", "report", "--json"];
    ConsolePage {
        id: "pentaho",
        title: "Pentaho → dbt",
        description: "Kettle packages translated into silver and gold dbt models (RFC 0200). Pick a \
                      package, then a step. Every step says how faithfully it was carried over: \
                      exact, rewritten (a rule a reviewer should read), llm-drafted, or gap (not \
                      translated, and the models that need it are disabled). Translating proposes \
                      a branch; merging it is the approval.",
        params: vec![
            PageParam {
                name: "package",
                label: "Package",
                source: "packages",
                rows: "/packages",
                key: "/package",
            },
            PageParam {
                name: "step",
                label: "Step",
                source: "steps",
                rows: "/steps",
                key: "/id",
            },
        ],
        panels: vec![
            Panel {
                id: "packages",
                title: "Packages",
                command: vec!["lakehouse", "pentaho", "scan", "--json"],
                view: View::Table {
                    rows: "/packages",
                    columns: vec![
                        Column::new("/package", "Package"),
                        Column::new("/transformations", "Transformations"),
                        Column::new("/jobs", "Jobs"),
                        Column::new("/steps", "Steps"),
                        Column::new("/unknown_count", "Unknown step types"),
                        Column::new("/unresolved_links", "Unresolved links"),
                        Column::badge("/translation", "Translation"),
                        Column::new("/translated/gap", "Gaps"),
                    ],
                },
                empty: "No Pentaho package: no observed directory holds a .ktr or .kjb (set \
                        [extensions.lakehouse] pentaho-packages in ekos.toml).",
                ..Default::default()
            },
            Panel {
                id: "summary",
                title: "Translation",
                command: report(),
                params: vec!["package"],
                view: View::Fields {
                    fields: vec![
                        Column::badge("/state", "State"),
                        Column::new("/summary/exact", "Exact"),
                        Column::new("/summary/rewritten", "Rewritten"),
                        Column::new("/summary/llm_drafted", "LLM-drafted"),
                        Column::new("/summary/gap", "Gap"),
                        Column::new("/summary/models", "Models"),
                        Column::new("/summary/enabled", "Enabled"),
                        Column::new("/summary/questions", "Questions"),
                    ],
                },
                empty: "Not translated yet: run Translate.",
            },
            Panel {
                id: "graph",
                title: "Transformations",
                command: report(),
                params: vec!["package"],
                view: View::Graph {
                    nodes: "/graph/nodes",
                    edges: "/graph/edges",
                    id: "/id",
                    label: "/label",
                    group: "/group",
                    badge: "/fidelity",
                    from: "/from",
                    to: "/to",
                },
                empty: "Not translated yet.",
            },
            Panel {
                id: "models",
                title: "Models",
                command: report(),
                params: vec!["package"],
                view: View::Table {
                    rows: "/models",
                    columns: vec![
                        Column::new("/name", "Model"),
                        Column::new("/layer", "Layer"),
                        Column::new("/transformation", "From"),
                        Column::new("/materialization", "Materialization"),
                        Column::badge("/fidelity", "Fidelity"),
                        Column::badge("/enabled", "Enabled"),
                        Column::new("/gaps/0", "Why disabled"),
                    ],
                },
                empty: "No models.",
            },
            Panel {
                id: "steps",
                title: "Steps (click one)",
                command: report(),
                params: vec!["package"],
                view: View::Table {
                    rows: "/steps",
                    columns: vec![
                        Column::new("/transformation", "Transformation"),
                        Column::new("/step", "Step"),
                        Column::new("/step_type", "Type"),
                        Column::new("/layer", "Layer"),
                        Column::badge("/fidelity", "Fidelity"),
                        Column::new("/gap", "Gap"),
                        Column::new("/notes_text", "Notes"),
                    ],
                },
                empty: "No steps.",
            },
            Panel {
                id: "step",
                title: "Step: original ↔ SQL",
                command: report(),
                params: vec!["package", "step"],
                view: View::Code {
                    left: "/step/original",
                    right: "/step/sql",
                    left_label: "Kettle (original)",
                    right_label: "Trino SQL",
                },
                empty: "This step has no text.",
            },
            Panel {
                id: "step-notes",
                title: "Step: fidelity and notes",
                command: report(),
                params: vec!["package", "step"],
                view: View::Fields {
                    fields: vec![
                        Column::badge("/step/fidelity", "Fidelity"),
                        Column::new("/step/gap", "Gap"),
                        Column::new("/step/notes_text", "Notes"),
                        Column::new("/step/models", "Models"),
                        Column::new("/step/line", "Line"),
                    ],
                },
                empty: "",
            },
            Panel {
                id: "questions",
                title: "Questions",
                command: report(),
                params: vec!["package"],
                view: View::Table {
                    rows: "/questions",
                    columns: vec![
                        Column::new("/transformation", "Transformation"),
                        Column::new("/step", "Step"),
                        Column::new("/question", "Question"),
                    ],
                },
                empty: "No questions: every table resolved.",
            },
            Panel {
                id: "proposal",
                title: "Proposal",
                command: report(),
                params: vec!["package"],
                view: View::Markdown {
                    pointer: "/proposal",
                },
                empty: "Not translated yet.",
            },
        ],
        actions: vec![
            Action {
                label: "Scan packages",
                command: vec!["lakehouse", "pentaho", "scan"],
                ..Default::default()
            },
            Action {
                label: "Translate",
                command: vec!["lakehouse", "pentaho", "translate"],
                binds: vec![("package", "package")],
                ..Default::default()
            },
        ],
    }
}

fn mode_name(m: &LoadMode) -> (&'static str, Option<String>) {
    match m {
        LoadMode::Full => ("full", None),
        LoadMode::Incremental { watermark, .. } => ("incremental", Some(watermark.clone())),
    }
}

/// `lakehouse tables --json`: the deployed plan (`lakehouse.lock.json`) and where the stack is.
pub fn tables(settings: &Settings, repo: &Path) -> Value {
    let settings_json = json!({
        "trino": settings.trino_url,
        "dagster": settings.dagster_url,
        "polaris": settings.polaris_health_url,
        "storage": settings.s3_url,
        "source": settings.source.as_ref().map(|s| format!(
            "{s}/{}", settings.source_database.as_deref().unwrap_or("?"))),
        "target_catalog": settings.target_catalog,
        "bronze_schedule": settings.bronze_schedule,
        "pipeline": repo.display().to_string(),
    });
    let Ok(plan) = read_lock(repo) else {
        return json!({"generated": false, "tables": [], "refused": [], "settings": settings_json});
    };
    // The deployed plan names the catalog even when the setting is left to its default.
    let mut settings_json = settings_json;
    settings_json["target_catalog"] = json!(plan.settings.target_catalog);
    let tables: Vec<Value> = plan
        .tables
        .iter()
        .map(|t| {
            let (mode, watermark) = mode_name(&t.decision.mode);
            let not_exact: Vec<String> = t
                .columns
                .iter()
                .filter(|c| c.lossiness != Lossiness::Exact)
                .map(|c| format!("{} ({:?})", c.column, c.lossiness))
                .collect();
            json!({
                "table": format!("{}.{}", t.schema, t.table),
                "mode": mode,
                "watermark": watermark,
                "primary_key": t.primary_key,
                "not_exact": not_exact,
                "columns": t.columns.len(),
                "evidence": t.decision.evidence,
            })
        })
        .collect();
    let refused: Vec<Value> = plan
        .refused
        .iter()
        .map(|r| json!({"table": format!("{}.{}", r.schema, r.table), "reasons": r.reasons}))
        .collect();
    json!({"generated": true, "tables": tables, "refused": refused, "settings": settings_json})
}

/// `lakehouse proposals --json`: the branches waiting for a merge (RFC 0194 schema changes,
/// RFC 0195 gold marts), from git alone.
pub fn proposals(repo: &Path) -> Value {
    let mut out = vec![];
    if repo.join(".git").exists() {
        for (prefix, kind) in [
            ("ekos/change/", "schema change"),
            ("ekos/gold/", "gold mart"),
        ] {
            let list = git(
                repo,
                &[
                    "for-each-ref",
                    "--format=%(refname:short)\t%(committerdate:iso-strict)\t%(subject)",
                    &format!("refs/heads/{prefix}"),
                ],
            )
            .unwrap_or_default();
            for line in list.lines() {
                let mut parts = line.splitn(3, '\t');
                let (Some(branch), Some(at), Some(subject)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let rest = branch.trim_start_matches(prefix);
                let subject_name = rest.rsplit_once('/').map(|(s, _)| s).unwrap_or(rest);
                out.push(json!({
                    "kind": kind,
                    "subject": subject_name,
                    "summary": subject.trim_start_matches("lakehouse: "),
                    "branch": branch,
                    "at": at,
                }));
            }
        }
    }
    json!({"proposals": out})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 0207: both pages pass EKOS's own checker against this binary's real catalogue
    /// (the check `ekos extensions --json` runs before the console serves a page).
    #[test]
    fn every_page_passes_the_console_checker() {
        let ext =
            ekos::extension::Extensions::new(vec![std::sync::Arc::new(crate::LakehouseExtension)]);
        let mut root = ext.extend_cli(ekos::app::cli_command()).unwrap();
        root.build();
        let cat = ekos::commands::catalogue::catalogue_with(&root, &ext.command_classes());
        let out = ekos::commands::extension_pages::describe(&ext, &cat).unwrap();
        let ids: Vec<&str> = out["extensions"][0]["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["lakehouse", "pentaho"]);
    }

    #[test]
    fn panels_only_read_and_name_the_new_commands() {
        let p = &pages()[0];
        for panel in &p.panels {
            assert_eq!(panel.command[0], "lakehouse");
            assert!(panel.command.ends_with(&["--json"]), "{:?}", panel.command);
        }
        let cmds: std::collections::BTreeSet<&str> =
            p.panels.iter().map(|x| x.command[1]).collect();
        assert_eq!(
            cmds,
            ["proposals", "status", "tables"].into_iter().collect()
        );
    }

    #[test]
    fn without_a_pipeline_there_is_nothing_but_the_settings() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings {
            trino_url: Some("http://t:8080".into()),
            ..Default::default()
        };
        let v = tables(&s, dir.path());
        assert_eq!(v["generated"], false);
        assert_eq!(v["tables"], json!([]));
        assert_eq!(v["settings"]["trino"], "http://t:8080");
        assert_eq!(proposals(dir.path()), json!({"proposals": []}));
    }

    #[test]
    fn proposal_branches_are_listed_with_their_subject() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let g = |args: &[&str]| {
            let st = std::process::Command::new("git")
                .current_dir(r)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        };
        g(&["init", "-q", "-b", "main"]);
        std::fs::write(r.join("a"), "1").unwrap();
        g(&["add", "a"]);
        g(&["commit", "-qm", "lakehouse: generate"]);
        g(&[
            "checkout",
            "-qb",
            "ekos/change/public.category/12e204d0d56e",
        ]);
        std::fs::write(r.join("a"), "2").unwrap();
        g(&[
            "commit",
            "-qam",
            "lakehouse: propose rename in public.category",
        ]);
        g(&["checkout", "-q", "main"]);
        let v = proposals(r);
        let p = &v["proposals"][0];
        assert_eq!(p["kind"], "schema change");
        assert_eq!(p["subject"], "public.category");
        assert_eq!(p["summary"], "propose rename in public.category");
        assert_eq!(p["branch"], "ekos/change/public.category/12e204d0d56e");
    }
}

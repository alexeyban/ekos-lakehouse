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
                Action {
                    label: "Run the pipeline",
                    command: vec!["lakehouse", "run"],
                    confirm: Some(
                        "Dagster runs the job now: every table is read from the source, loaded \
                         into bronze, reconciled, and silver is built.",
                    ),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        requirements_page(),
        marts_page(),
        extraction_page(),
        pentaho_page(),
    ]
}

/// EKOS RFC 0228: step 7 — each approved requirement's mart from proposal to reconciliation.
fn marts_page() -> ConsolePage {
    let show = || vec!["lakehouse", "marts", "show", "--json"];
    let act = |label: &'static str, verb: &'static str| Action {
        label,
        command: vec!["lakehouse", "marts", verb],
        binds: vec![("mart", "mart")],
        ..Default::default()
    };
    ConsolePage {
        id: "marts",
        title: "Marts",
        description: "Step 7: each approved requirement becomes a gold mart. Propose drafts the dbt \
                      model on a branch (nothing invented: every part of it comes from the \
                      requirement); Merge is your approval of the generated model; Build runs it \
                      and its tests through Dagster; Reconcile compares each measure's total with \
                      silver's. The SQL is a template that may need tuning: terms marked drafted \
                      were written by a model and need a person to read them.",
        params: vec![PageParam {
            name: "mart",
            label: "Mart",
            source: "marts",
            rows: "/marts",
            key: "/mart",
        }],
        panels: vec![
            Panel {
                id: "marts",
                title: "Marts (click one)",
                command: show(),
                view: View::Table {
                    rows: "/marts",
                    columns: vec![
                        Column::new("/mart", "Mart"),
                        Column::badge("/state", "State"),
                        Column::new("/branch", "Proposal"),
                        Column::new("/last_build", "Last build"),
                        Column::new("/reconciled_at", "Reconciled at"),
                    ],
                },
                empty: "No mart yet: approve a requirement on the Requirements page.",
                ..Default::default()
            },
            Panel {
                id: "mart",
                title: "Mart",
                command: show(),
                params: vec!["mart"],
                view: View::Markdown {
                    pointer: "/selected/markdown",
                },
                empty: "",
            },
            Panel {
                id: "sql",
                title: "Requirement and generated SQL",
                command: show(),
                params: vec!["mart"],
                view: View::Code {
                    left: "/selected/description",
                    right: "/selected/sql",
                    left_label: "Approved requirement",
                    right_label: "dbt model (gold)",
                },
                empty: "",
            },
            Panel {
                id: "terms",
                title: "Terms: exact or drafted",
                command: show(),
                params: vec!["mart"],
                view: View::Table {
                    rows: "/selected/terms",
                    columns: vec![
                        Column::new("/section", "Section"),
                        Column::new("/term", "Term"),
                        Column::new("/written", "Written"),
                        Column::badge("/fidelity", "Fidelity"),
                        Column::new("/how", "How"),
                    ],
                },
                empty: "",
            },
            Panel {
                id: "reconciliation",
                title: "Reconciliation with silver",
                command: show(),
                params: vec!["mart"],
                view: View::Table {
                    rows: "/selected/reconciliation",
                    columns: vec![
                        Column::new("/measure", "Measure"),
                        Column::new("/mart_total", "Mart total"),
                        Column::new("/silver_total", "Silver total"),
                        Column::badge("/equal", "Result"),
                    ],
                },
                empty: "Not reconciled yet: Build, then Reconcile.",
            },
            Panel {
                id: "tests",
                title: "Tests",
                command: show(),
                params: vec!["mart"],
                view: View::Table {
                    rows: "/selected/tests",
                    columns: vec![Column::new("/test", "Test"), Column::new("/what", "Checks")],
                },
                empty: "",
            },
            Panel {
                id: "lineage",
                title: "Lineage",
                command: show(),
                params: vec!["mart"],
                view: View::Graph {
                    nodes: "/selected/graph/nodes",
                    edges: "/selected/graph/edges",
                    id: "/id",
                    label: "/label",
                    group: "/group",
                    badge: "/fidelity",
                    from: "/from",
                    to: "/to",
                },
                empty: "",
            },
        ],
        actions: vec![
            act("Propose", "propose"),
            Action {
                label: "Merge",
                command: vec!["lakehouse", "marts", "merge"],
                binds: vec![("mart", "mart")],
                inputs: vec![("note", "Note (optional)")],
                confirm: Some(
                    "You approve the generated model as shown, under your name: its proposal is \
                     merged into the pipeline and the next build makes it a table.",
                ),
                ..Default::default()
            },
            act("Build", "build"),
            act("Reconcile", "reconcile"),
        ],
    }
}

/// EKOS RFC 0215: step 4 — mart requirements, drafted from text by the configured model or
/// written as descriptions, resolved by EKOS against the deployed plan, changed and approved.
fn requirements_page() -> ConsolePage {
    let show = || vec!["lakehouse", "requirements", "show", "--json"];
    let edit = |label: &'static str, inputs: Vec<(&'static str, &'static str)>, flags| Action {
        label,
        command: vec!["lakehouse", "requirements", "edit"],
        binds: vec![("mart", "mart")],
        inputs,
        flags,
        ..Default::default()
    };
    ConsolePage {
        id: "requirements",
        title: "Requirements",
        description: "Step 4: describe a mart in plain words and the configured model drafts its \
                      metadata (grain, dimensions, measures, filters); EKOS then resolves every \
                      term against the approved plan. What does not resolve, or could mean more \
                      than one thing, is a question for you, never for the model: answer it by \
                      changing the term. Drafts can be wrong; nothing is built until every term \
                      resolves and you approve the requirement.",
        params: vec![PageParam {
            name: "mart",
            label: "Mart",
            source: "marts",
            rows: "/marts",
            key: "/mart",
        }],
        panels: vec![
            Panel {
                id: "marts",
                title: "Requirements (click one)",
                command: show(),
                view: View::Table {
                    rows: "/marts",
                    columns: vec![
                        Column::new("/mart", "Mart"),
                        Column::badge("/state", "State"),
                        Column::new("/source", "From"),
                        Column::new("/terms", "Terms"),
                        Column::new("/questions", "Questions"),
                        Column::new("/needs", "Silver tables"),
                        Column::new("/approved_by", "Approved by"),
                        Column::new("/approved_at", "Approved at"),
                    ],
                },
                empty: "No requirement yet: Draft from text.",
                ..Default::default()
            },
            Panel {
                id: "mart",
                title: "Requirement",
                command: show(),
                params: vec!["mart"],
                view: View::Markdown {
                    pointer: "/selected/markdown",
                },
                empty: "",
            },
            Panel {
                id: "terms",
                title: "Terms: what each resolved to",
                command: show(),
                params: vec!["mart"],
                view: View::Table {
                    rows: "/selected/terms",
                    columns: vec![
                        Column::new("/section", "Section"),
                        Column::new("/term", "Term"),
                        Column::new("/written", "Written"),
                        Column::badge("/resolved", "Resolved"),
                        Column::new("/how", "How"),
                        Column::new("/question", "Question"),
                        Column::new("/candidates", "Candidates"),
                    ],
                },
                empty: "",
            },
            Panel {
                id: "needs",
                title: "Silver tables the requirements read",
                command: show(),
                view: View::Table {
                    rows: "/needs_rows",
                    columns: vec![Column::new("/table", "Table")],
                },
                empty: "None yet: a requirement reads tables once every term resolves.",
                ..Default::default()
            },
        ],
        actions: vec![
            Action {
                label: "Draft from text",
                command: vec!["lakehouse", "requirements", "draft"],
                inputs: vec![
                    ("mart", "Mart name (lowercase letters, digits, _)"),
                    ("text", "What should it show? (plain words)"),
                ],
                ..Default::default()
            },
            edit(
                "Change a term",
                vec![("term", "Term"), ("value", "New definition")],
                vec![],
            ),
            edit(
                "Add a dimension",
                vec![("term", "Name"), ("value", "Definition")],
                vec!["dimension"],
            ),
            edit(
                "Add a measure",
                vec![("term", "Name"), ("value", "Definition")],
                vec!["measure"],
            ),
            edit("Remove a term", vec![("drop-term", "Term")], vec![]),
            edit("Add a filter", vec![("filter", "Filter")], vec![]),
            edit(
                "Remove a filter",
                vec![("drop-filter", "Filter, as written")],
                vec![],
            ),
            Action {
                label: "Approve",
                command: vec!["lakehouse", "requirements", "approve"],
                binds: vec![("mart", "mart")],
                inputs: vec![("note", "Note (optional)")],
                confirm: Some(
                    "You approve this requirement as shown, under your name: it is committed to \
                     the pipeline repository as marts/<mart>.md and recorded in the ledger with \
                     its digest. Gold is built only from approved requirements.",
                ),
                ..Default::default()
            },
        ],
    }
}

/// EKOS RFC 0226: step 5 — the proposed extraction plan with its evidence, the operator's changes
/// per table (with a reason) and the approval. Panels read the state directory only.
fn extraction_page() -> ConsolePage {
    let show = || vec!["lakehouse", "extraction", "show", "--json"];
    let change = |label: &'static str, flag: &'static str| Action {
        label,
        command: vec!["lakehouse", "extraction", "change"],
        binds: vec![("table", "table")],
        flags: vec![flag],
        inputs: vec![("reason", "Why (kept with the change)")],
        ..Default::default()
    };
    ConsolePage {
        id: "extraction",
        title: "Extraction",
        description: "Step 5: how each table lands in bronze, with the evidence for it — the load \
                      mode, how deletes are found, type mappings, masks — and step 3's decisions \
                      applied (scope, PII masks, write-behaviour overrides; only subjects with a \
                      valid sign-off). Pick a table to change it; a change keeps its reason and \
                      who made it. Approve the plan to build it: with require-plan-approval, \
                      Generate builds only the approved plan.",
        params: vec![PageParam {
            name: "table",
            label: "Table",
            source: "tables",
            rows: "/tables",
            key: "/table",
        }],
        panels: vec![
            Panel {
                id: "summary",
                title: "The plan",
                command: show(),
                view: View::Fields {
                    fields: vec![
                        Column::badge("/summary/state", "State"),
                        Column::new("/summary/approved_by", "Approved by"),
                        Column::new("/summary/approved_at", "Approved at"),
                        Column::new("/summary/planned_at", "Planned at"),
                        Column::new("/summary/digest", "Digest"),
                        Column::new(
                            "/summary/incremental_every_run",
                            "Incremental, deletes every run",
                        ),
                        Column::new(
                            "/summary/incremental_audited",
                            "Incremental, deletes audited",
                        ),
                        Column::new("/summary/incremental_other", "Incremental, other"),
                        Column::new("/summary/full", "Full"),
                        Column::new("/summary/refused", "Refused"),
                        Column::new("/summary/excluded", "Out of scope"),
                        Column::new("/summary/masked_columns", "Masked columns"),
                        Column::new("/summary/changes", "Changes"),
                        Column::new("/summary/stale_changes", "Changes not applied"),
                        Column::new("/summary/notes", "Notes"),
                    ],
                },
                empty: "No plan yet: run Plan (it reads the source).",
                ..Default::default()
            },
            Panel {
                id: "step3",
                title: "Step 3 decisions used",
                command: show(),
                view: View::Table {
                    rows: "/step3",
                    columns: vec![
                        Column::new("/subject", "Subject"),
                        Column::badge("/used", "Used"),
                        Column::new("/why", "Why"),
                    ],
                },
                empty: "No plan yet.",
                ..Default::default()
            },
            Panel {
                id: "tables",
                title: "Tables (click one)",
                command: show(),
                view: View::Table {
                    rows: "/tables",
                    columns: vec![
                        Column::new("/table", "Table"),
                        Column::badge("/mode", "Mode"),
                        Column::new("/watermark", "Watermark"),
                        Column::new("/deletes", "Deletes checked"),
                        Column::new("/kind", "A found delete is"),
                        Column::new("/purge_policy", "Purged rows"),
                        Column::badge("/contradicts_code", "Would contradict the code"),
                        Column::new("/not_exact", "Columns not exact"),
                        Column::new("/masked", "Masked"),
                        Column::new("/change", "Changed by the operator"),
                        Column::badge("/since_approval", "Since the approval"),
                    ],
                },
                empty: "No plan yet: run Plan.",
                ..Default::default()
            },
            Panel {
                id: "table",
                title: "Table: evidence and changes",
                command: show(),
                params: vec!["table"],
                view: View::Markdown {
                    pointer: "/selected/markdown",
                },
                empty: "",
            },
            Panel {
                id: "columns",
                title: "Table: columns",
                command: show(),
                params: vec!["table"],
                view: View::Table {
                    rows: "/selected/columns",
                    columns: vec![
                        Column::new("/column", "Column"),
                        Column::new("/source", "Source type"),
                        Column::new("/iceberg", "Iceberg type"),
                        Column::badge("/lossiness", "Lossiness"),
                        Column::new("/rule", "Rule"),
                        Column::badge("/mask", "Mask"),
                    ],
                },
                empty: "",
            },
            Panel {
                id: "changes",
                title: "Changes",
                command: show(),
                view: View::Table {
                    rows: "/changes",
                    columns: vec![
                        Column::new("/table", "Table"),
                        Column::new("/change", "Change"),
                        Column::new("/reason", "Why"),
                        Column::new("/by", "By"),
                        Column::new("/at", "At"),
                        Column::badge("/state", "State"),
                        Column::new("/why_stale", "Not applied because"),
                    ],
                },
                empty: "No changes: the plan is as proposed.",
                ..Default::default()
            },
            Panel {
                id: "refused",
                title: "Refused",
                command: show(),
                view: View::Table {
                    rows: "/refused",
                    columns: vec![
                        Column::new("/table", "Table"),
                        Column::new("/reasons", "Why"),
                    ],
                },
                empty: "No table is refused.",
                ..Default::default()
            },
            Panel {
                id: "excluded",
                title: "Out of scope (step 3)",
                command: show(),
                view: View::Table {
                    rows: "/excluded",
                    columns: vec![
                        Column::new("/table", "Table"),
                        Column::new("/reason", "Why"),
                    ],
                },
                empty: "Nothing out of scope, or the scope is not signed off.",
                ..Default::default()
            },
        ],
        actions: vec![
            Action {
                label: "Plan",
                command: vec!["lakehouse", "plan"],
                ..Default::default()
            },
            change("Load in full", "full"),
            change("Load incrementally", "incremental"),
            change("Look for deletes every run", "deletes-every-run"),
            change("Audit deletes", "deletes-audit"),
            change("Keep purged rows", "keep-purged"),
            change("Remove purged rows", "remove-purged"),
            Action {
                label: "Undo changes",
                command: vec!["lakehouse", "extraction", "change"],
                binds: vec![("table", "table")],
                flags: vec!["undo"],
                ..Default::default()
            },
            Action {
                label: "Approve plan",
                command: vec!["lakehouse", "extraction", "approve"],
                inputs: vec![("note", "Note (optional)")],
                confirm: Some(
                    "You approve the plan as shown, every table and change, under your name. \
                     The approval is recorded in the ledger with the plan's digest; with \
                     require-plan-approval, Generate builds only this plan.",
                ),
                ..Default::default()
            },
        ],
    }
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
            Action {
                label: "Translate, a model drafting the code steps",
                command: vec!["lakehouse", "pentaho", "translate"],
                binds: vec![("package", "package")],
                flags: vec!["allow-metered"],
                confirm: Some(
                    "A model drafts the JavaScript and Java steps the translator cannot follow (RFC \
                     0205). Through a paid route each call is billed and logged; the tenant must \
                     allow metered calls. Drafts are checked, labelled llm-drafted, and proposed \
                     for review like everything else.",
                ),
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
        assert_eq!(
            ids,
            vec![
                "lakehouse",
                "requirements",
                "marts",
                "extraction",
                "pentaho"
            ]
        );
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

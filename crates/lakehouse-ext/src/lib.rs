//! EKOS Lakehouse as an EKOS extension (RFC 0190, seam: RFC 0149 / 0191).
//!
//! EKOS stays in the control plane: it reads the source, generates pipeline code into a Git
//! repository the user owns and records every run and check as ledger facts. Trino moves the
//! data and the orchestrator schedules it; nothing here ever moves a row.

pub mod check;
pub mod console;
pub mod deletes;
pub mod doctor;
pub mod extraction;
pub mod facts;
pub mod gold_cmd;
pub mod install;
pub mod monitor;
pub mod pentaho_cmd;
pub mod plan_cmd;
pub mod reconcile;
pub mod settings;
pub mod source;
pub mod source_oracle;
pub mod sync;
pub mod workflow;

use async_trait::async_trait;
use ekos::commands::catalogue::Class;
use ekos::extension::{CommandContext, EkosExtension};

/// The extension's name, and the `[extensions.<name>]` table its settings live in.
pub const NAME: &str = "lakehouse";

pub struct LakehouseExtension;

#[async_trait(?Send)]
impl EkosExtension for LakehouseExtension {
    fn name(&self) -> &'static str {
        NAME
    }

    fn mcp_tools(&self, _config: &ekos_compiler_core::EkosConfig) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "name": "ekos_pipeline_status",
                "description": "EKOS Lakehouse: every table's health per layer (bronze, silver, gold) — healthy, late, unverified (a planted reconciliation control missed), failing, or schema_pending (a source change awaits approval) — with the facts that put it there and, for a failing table, the probable cause. Read-only: it cannot rerun, merge or approve anything.",
                "inputSchema": {"type": "object", "properties": {
                    "late_after_minutes": {"type": "integer", "description": "A table with no successful run for this long is late (default 120)"},
                    "only_unhealthy": {"type": "boolean", "description": "Leave out healthy tables"}
                }}
            }),
            serde_json::json!({
                "name": "ekos_pentaho_translation",
                "description": "EKOS Lakehouse: the latest translation of each Pentaho package into silver and gold dbt models — per model its layer, materialization, fidelity (exact, rewritten, llm-drafted, gap) and whether it is enabled; per step its notes, gap reason, original Kettle text and generated SQL; the open questions. Read-only: it cannot translate, merge or approve.",
                "inputSchema": {"type": "object", "properties": {
                    "package": {"type": "string", "description": "Only this package"},
                    "step": {"type": "string", "description": "Only steps with this name (with original text and SQL)"}
                }}
            }),
            serde_json::json!({
                "name": "ekos_table_health",
                "description": "EKOS Lakehouse: one table's health per layer with its evidence, recent runs, reconciliations (tiers, divergences, planted control) and schema changes. Read-only.",
                "inputSchema": {"type": "object", "required": ["table"], "properties": {
                    "table": {"type": "string", "description": "schema.table, e.g. public.orders"},
                    "late_after_minutes": {"type": "integer"}
                }}
            }),
        ]
    }

    fn mcp_tool_is_scoped(&self, name: &str) -> bool {
        // All read only through the store they are handed (RFC 0191).
        matches!(
            name,
            "ekos_pipeline_status" | "ekos_table_health" | "ekos_pentaho_translation"
        )
    }

    fn call_mcp_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        ledger: &dyn ekos_ledger::KnowledgeStore,
    ) -> Option<anyhow::Result<serde_json::Value>> {
        let late = args["late_after_minutes"].as_i64().unwrap_or(120);
        match name {
            "ekos_pipeline_status" => Some((|| {
                let h = monitor::health(ledger, late)?;
                let only = args["only_unhealthy"].as_bool().unwrap_or(false);
                let shown: Vec<_> = h
                    .iter()
                    .filter(|x| !only || x.state != ekos_lakehouse_health::State::Healthy)
                    .collect();
                Ok(
                    serde_json::json!({"tables": shown, "summary": monitor::render(&h).lines().last()}),
                )
            })()),
            "ekos_pentaho_translation" => Some((|| {
                let package = args["package"].as_str();
                let step = args["step"].as_str();
                let mut out = Vec::new();
                for mut t in facts::translations(ledger)? {
                    if package.is_some_and(|p| t["package"].as_str() != Some(p)) {
                        continue;
                    }
                    if let Some(steps) = t["report"]["steps"].as_array_mut() {
                        match step {
                            Some(name) => steps.retain(|s| s["step"].as_str() == Some(name)),
                            // Without a step: no code and SQL, to keep the answer short.
                            None => {
                                for s in steps.iter_mut() {
                                    if let Some(o) = s.as_object_mut() {
                                        o.remove("original");
                                        o.remove("sql");
                                    }
                                }
                            }
                        }
                    }
                    out.push(t);
                }
                Ok(serde_json::json!({ "translations": out }))
            })()),
            "ekos_table_health" => Some((|| {
                let table = args["table"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("`table` is required"))?;
                let f = monitor::facts(ledger)?;
                let h: Vec<_> = ekos_lakehouse_health::evaluate(&f, monitor::now(), late)
                    .into_iter()
                    .filter(|x| x.table == table)
                    .collect();
                anyhow::ensure!(!h.is_empty(), "no lakehouse facts for {table}");
                let mut runs: Vec<_> = f.runs.iter().filter(|r| r.table == table).collect();
                runs.sort_by_key(|r| std::cmp::Reverse(r.at));
                let mut recon: Vec<_> = f
                    .reconciliations
                    .iter()
                    .filter(|r| r.table == table)
                    .collect();
                recon.sort_by_key(|r| std::cmp::Reverse(r.at));
                let changes: Vec<_> = f.changes.iter().filter(|c| c.table == table).collect();
                Ok(serde_json::json!({
                    "health": h,
                    "recent_runs": runs.into_iter().take(10).collect::<Vec<_>>(),
                    "recent_reconciliations": recon.into_iter().take(5).collect::<Vec<_>>(),
                    "schema_changes": changes,
                }))
            })()),
            _ => None,
        }
    }

    fn commands(&self) -> Vec<clap::Command> {
        vec![
            clap::Command::new(NAME)
                .about("EKOS Lakehouse: PostgreSQL → Iceberg pipelines (RFC 0190)")
                .subcommand_required(true)
                .subcommand(
                    clap::Command::new("plan")
                        .about(
                            "Read the source and show what bronze would be: each table's \
                                mode, mappings and refusals (RFC 0192)",
                        )
                        .arg(
                            clap::Arg::new("json")
                                .long("json")
                                .action(clap::ArgAction::SetTrue)
                                .help("The full plan, as lakehouse.lock.json would hold it"),
                        ),
                )
                .subcommand(
                    clap::Command::new("generate")
                        .about(
                            "Write bronze's SQL and lakehouse.lock.json into a pipeline \
                                repository (RFC 0192)",
                        )
                        .arg(
                            clap::Arg::new("out")
                                .long("out")
                                .value_parser(clap::value_parser!(std::path::PathBuf))
                                .help("The pipeline repository (default: `pipeline-dir`)"),
                        )
                        .arg(
                            clap::Arg::new("from-lock")
                                .long("from-lock")
                                .action(clap::ArgAction::SetTrue)
                                .help(
                                    "Re-render from the deployed plan (lakehouse.lock.json) without \
                                     reading the source: after an upgrade (RFC 0199)",
                                ),
                        ),
                )
                .subcommand(
                    clap::Command::new("reconcile")
                        .about(
                            "Reconcile bronze against the source at V1-V3 with a planted \
                                control, and record the result (RFC 0192)",
                        )
                        .arg(
                            clap::Arg::new("table")
                                .long("table")
                                .action(clap::ArgAction::Append)
                                .help("schema.table; repeat it. Default: every planned table"),
                        )
                        .arg(
                            clap::Arg::new("run-id")
                                .long("run-id")
                                .help("The orchestrator's run id (default: manual-<time>)"),
                        )
                        .arg(
                            clap::Arg::new("json")
                                .long("json")
                                .action(clap::ArgAction::SetTrue)
                                .help("Machine-readable output"),
                        ),
                )
                .subcommand(
                    clap::Command::new("deletes")
                        .about(
                            "Find rows deleted at the source by a bucketed key diff, tombstone them \
                                in bronze, record the counts (RFC 0225)",
                        )
                        .arg(
                            clap::Arg::new("table")
                                .long("table")
                                .required(true)
                                .help("schema.table"),
                        )
                        .arg(
                            clap::Arg::new("run-id")
                                .long("run-id")
                                .help("The orchestrator's run id (default: manual-<time>)"),
                        )
                        .arg(
                            clap::Arg::new("accept")
                                .long("accept")
                                .action(clap::ArgAction::SetTrue)
                                .help("Apply what the mass-delete guard stopped: a person knows why"),
                        )
                        .arg(
                            clap::Arg::new("force")
                                .long("force")
                                .action(clap::ArgAction::SetTrue)
                                .help("Check now, even when an audited table is not due"),
                        ),
                )
                .subcommand(
                    clap::Command::new("sync")
                        .about(
                            "Compare the live source with the deployed plan: commit safe \
                                changes, propose the rest on branches (RFC 0194)",
                        )
                        .arg(
                            clap::Arg::new("json")
                                .long("json")
                                .action(clap::ArgAction::SetTrue)
                                .help("Machine-readable output"),
                        ),
                )
                .subcommand(
                    clap::Command::new("gold")
                        .about("Gold marts from written descriptions (RFC 0195)")
                        .subcommand_required(true)
                        .subcommand(
                            clap::Command::new("draft")
                                .about(
                                    "Resolve a mart description's terms; ask about what does \
                                        not resolve, or propose the mart on a branch",
                                )
                                .arg(
                                    clap::Arg::new("description")
                                        .required(true)
                                        .value_parser(clap::value_parser!(std::path::PathBuf))
                                        .help("marts/<name>.md"),
                                ),
                        ),
                )
                .subcommand(
                    clap::Command::new("pentaho")
                        .about("Pentaho packages into silver and gold dbt models (RFC 0200)")
                        .subcommand_required(true)
                        .subcommand(
                            clap::Command::new("translate")
                                .about(
                                    "Translate a package into silver and gold dbt models and propose \
                                     them on a branch (RFC 0204); Trino plans every model first",
                                )
                                .arg(clap::Arg::new("package").required(true).help("The package's name (see `pentaho scan`)"))
                                .arg(
                                    clap::Arg::new("out")
                                        .long("out")
                                        .value_parser(clap::value_parser!(std::path::PathBuf))
                                        .help("Write into this directory instead of proposing a branch"),
                                )
                                .arg(
                                    clap::Arg::new("allow-metered")
                                        .long("allow-metered")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Let a paid model draft the code steps the translator cannot follow (RFC 0205); an Ollama route needs no flag"),
                                )
                                .arg(
                                    clap::Arg::new("no-explain")
                                        .long("no-explain")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Do not plan the models on Trino"),
                                )
                                .arg(clap::Arg::new("json").long("json").action(clap::ArgAction::SetTrue).help("Machine-readable output")),
                        )
                        .subcommand(
                            clap::Command::new("report")
                                .about("The latest translation of each package: models, steps, fidelity, gaps (RFC 0204)")
                                .arg(clap::Arg::new("package").long("package").help("One package: its models, steps, graph and proposal"))
                                .arg(clap::Arg::new("step").long("step").requires("package").help("With --package: one step's original text and SQL (its id, transformation:step)"))
                                .arg(clap::Arg::new("json").long("json").action(clap::ArgAction::SetTrue).help("Machine-readable output")),
                        )
                        .subcommand(
                            clap::Command::new("scan")
                                .about(
                                    "Read every Pentaho package: transformations, jobs, steps by \
                                     category, links between files and what could not be read",
                                )
                                .arg(
                                    clap::Arg::new("json")
                                        .long("json")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Machine-readable output"),
                                ),
                        ),
                )
                .subcommand(
                    clap::Command::new("reload")
                        .about(
                            "Load a table again from scratch on its next run: bronze is \
                                emptied and every source row loaded (RFC 0194)",
                        )
                        .arg(
                            clap::Arg::new("table")
                                .long("table")
                                .required(true)
                                .action(clap::ArgAction::Append)
                                .help("schema.table; repeat it"),
                        )
                        .arg(
                            clap::Arg::new("cancel")
                                .long("cancel")
                                .action(clap::ArgAction::SetTrue)
                                .help("Withdraw a reload not yet run"),
                        ),
                )
                .subcommand(
                    clap::Command::new("status")
                        .about(
                            "Every table's health per layer, with the facts behind it (RFC 0196)",
                        )
                        .arg(
                            clap::Arg::new("json")
                                .long("json")
                                .action(clap::ArgAction::SetTrue)
                                .help("Machine-readable output"),
                        ),
                )
                .subcommand(
                    clap::Command::new("install")
                        .about(
                            "Install the stack, a first project and the web console under the EKOS \
                             home, in one command (RFC 0199). Without --source: a sample `shop` \
                             database in the stack's own PostgreSQL",
                        )
                        .arg(clap::Arg::new("source").long("source").value_name("HOST:PORT/DATABASE")
                            .help("Your PostgreSQL. The password comes from --source-password-env"))
                        .arg(clap::Arg::new("source-user").long("source-user").default_value("ekos")
                            .help("A role that can read the source (a read-only one is the guarantee)"))
                        .arg(clap::Arg::new("source-password-env").long("source-password-env")
                            .default_value("EKOS_LH_SOURCE_PASSWORD")
                            .help("The variable holding the source password during the install"))
                        .arg(clap::Arg::new("name").long("name")
                            .help("The project's name (default: the database's)"))
                        .arg(clap::Arg::new("schemas").long("schemas").value_delimiter(',')
                            .help("Only these schemas (comma-separated; default: all)"))
                        .arg(clap::Arg::new("no-console").long("no-console").action(clap::ArgAction::SetTrue)
                            .help("Do not set up the web console"))
                        .arg(clap::Arg::new("no-open").long("no-open").action(clap::ArgAction::SetTrue)
                            .help("Do not open the browser"))
                        .arg(clap::Arg::new("at-login").long("at-login").action(clap::ArgAction::SetTrue)
                            .help("Start the console when you log in"))
                        .arg(clap::Arg::new("no-first-run").long("no-first-run").action(clap::ArgAction::SetTrue)
                            .help("Do not run the pipeline once after installing"))
                        .arg(clap::Arg::new("console-port").long("console-port").default_value("8000")
                            .value_parser(clap::value_parser!(u16)))
                        .arg(clap::Arg::new("uninstall").long("uninstall").action(clap::ArgAction::SetTrue)
                            .help("Stop and remove the stack (projects stay)"))
                        .arg(clap::Arg::new("purge").long("purge").action(clap::ArgAction::SetTrue)
                            .requires("uninstall")
                            .help("With --uninstall: also delete the stack's data volumes")),
                )
                .subcommand(
                    clap::Command::new("extraction")
                        .about(
                            "Step 5: the extraction plan with step 3's decisions, the operator's \
                             changes per table, and its approval (EKOS RFC 0226)",
                        )
                        .subcommand_required(true)
                        .subcommand(
                            clap::Command::new("show")
                                .about(
                                    "The proposed plan as it would be built, the changes and the \
                                     approval, from the state directory (no source read)",
                                )
                                .arg(
                                    clap::Arg::new("json")
                                        .long("json")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Machine-readable output"),
                                )
                                .arg(
                                    clap::Arg::new("table")
                                        .long("table")
                                        .help("Also show this table's evidence and columns"),
                                ),
                        )
                        .subcommand(
                            clap::Command::new("change")
                                .about(
                                    "Change how one table is extracted; applied to every later \
                                     plan until undone, used once the plan is approved",
                                )
                                .arg(
                                    clap::Arg::new("table")
                                        .long("table")
                                        .required(true)
                                        .help("`schema.table`, or the table's name when it is unique"),
                                )
                                .arg(
                                    clap::Arg::new("full")
                                        .long("full")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Load the table in full"),
                                )
                                .arg(
                                    clap::Arg::new("incremental")
                                        .long("incremental")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Load it incrementally on the application watermark (needs --reason when the facts keep it full)"),
                                )
                                .arg(
                                    clap::Arg::new("deletes-every-run")
                                        .long("deletes-every-run")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Look for its deletes on every run"),
                                )
                                .arg(
                                    clap::Arg::new("deletes-audit")
                                        .long("deletes-audit")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Audit its deletes periodically"),
                                )
                                .arg(
                                    clap::Arg::new("keep-purged")
                                        .long("keep-purged")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Keep rows the source purges"),
                                )
                                .arg(
                                    clap::Arg::new("remove-purged")
                                        .long("remove-purged")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Remove rows the source purges"),
                                )
                                .arg(
                                    clap::Arg::new("undo")
                                        .long("undo")
                                        .action(clap::ArgAction::SetTrue)
                                        .help("Remove every change to the table"),
                                )
                                .group(
                                    clap::ArgGroup::new("what")
                                        .args([
                                            "full",
                                            "incremental",
                                            "deletes-every-run",
                                            "deletes-audit",
                                            "keep-purged",
                                            "remove-purged",
                                            "undo",
                                        ])
                                        .required(true),
                                )
                                .arg(clap::Arg::new("reason").long("reason").help("Why"))
                                .arg(
                                    clap::Arg::new("by")
                                        .long("as")
                                        .help("Who decides (default: $USER; the console sets it)"),
                                ),
                        )
                        .subcommand(
                            clap::Command::new("approve")
                                .about(
                                    "Approve the plan as it would be built now: a ledger fact with \
                                     its digest; `generate` under require-plan-approval builds only it",
                                )
                                .arg(clap::Arg::new("note").long("note").help("A note kept with it"))
                                .arg(
                                    clap::Arg::new("by")
                                        .long("as")
                                        .help("Who approves (default: $USER; the console sets it)"),
                                ),
                        ),
                )
                .subcommand(
                    clap::Command::new("tables")
                        .about(
                            "The deployed plan: each table's load mode, watermark and columns not \
                             landed exactly, the refused tables and where the stack is (RFC 0199)",
                        )
                        .arg(
                            clap::Arg::new("json")
                                .long("json")
                                .action(clap::ArgAction::SetTrue)
                                .help("Machine-readable output"),
                        ),
                )
                .subcommand(
                    clap::Command::new("proposals")
                        .about(
                            "Schema changes and gold marts waiting for a merge, from the pipeline \
                             repository's branches (RFC 0199)",
                        )
                        .arg(
                            clap::Arg::new("json")
                                .long("json")
                                .action(clap::ArgAction::SetTrue)
                                .help("Machine-readable output"),
                        ),
                )
                .subcommand(
                    clap::Command::new("alert")
                        .about("Post each health transition once to the alert webhook (RFC 0196)"),
                )
                .subcommand(
                    clap::Command::new("metrics").about(
                        "Push health as OpenTelemetry metrics to the OTLP endpoint (RFC 0196)",
                    ),
                )
                .subcommand(
                    clap::Command::new("check")
                        .about(
                            "Measure the generated project: lineage through the compiled \
                                ledger, test and documentation coverage, drift from generated, \
                                lint (RFC 0193)",
                        )
                        .arg(
                            clap::Arg::new("lint")
                                .long("lint")
                                .action(clap::ArgAction::SetTrue)
                                .help("Also run sqlfluff (must be on PATH)"),
                        )
                        .arg(
                            clap::Arg::new("json")
                                .long("json")
                                .action(clap::ArgAction::SetTrue)
                                .help("Machine-readable output"),
                        ),
                )
                .subcommand(
                    clap::Command::new("run-report")
                        .about("Record one load run reported by the orchestrator (RFC 0192)")
                        .arg(
                            clap::Arg::new("table")
                                .long("table")
                                .required_unless_present("dbt-results")
                                .help("schema.table"),
                        )
                        .arg(
                            clap::Arg::new("run-id")
                                .long("run-id")
                                .required(true)
                                .help("The run id"),
                        )
                        .arg(
                            clap::Arg::new("status")
                                .long("status")
                                .required_unless_present("dbt-results")
                                .value_parser(["ok", "failed"])
                                .help("How the load ended"),
                        )
                        .arg(
                            clap::Arg::new("layer")
                                .long("layer")
                                .default_value("bronze")
                                .value_parser(["bronze", "silver", "gold"])
                                .help("The layer the run wrote"),
                        )
                        .arg(
                            clap::Arg::new("dbt-results")
                                .long("dbt-results")
                                .value_parser(clap::value_parser!(std::path::PathBuf))
                                .help(
                                    "dbt's target/ directory: record every model's build and tests",
                                ),
                        )
                        .arg(
                            clap::Arg::new("rows")
                                .long("rows")
                                .value_parser(clap::value_parser!(i64))
                                .help("Rows written"),
                        )
                        .arg(
                            clap::Arg::new("duration-ms")
                                .long("duration-ms")
                                .value_parser(clap::value_parser!(u64))
                                .help("Load duration"),
                        )
                        .arg(
                            clap::Arg::new("snapshot-id")
                                .long("snapshot-id")
                                .help("Iceberg snapshot written"),
                        )
                        .arg(
                            clap::Arg::new("message")
                                .long("message")
                                .help("Error text for a failed run"),
                        ),
                )
                .subcommand(
                    clap::Command::new("doctor")
                        .about(
                            "Check that every service of the lakehouse stack in \
                             [extensions.lakehouse] answers",
                        )
                        .arg(
                            clap::Arg::new("json")
                                .long("json")
                                .action(clap::ArgAction::SetTrue)
                                .help("Machine-readable output"),
                        ),
                ),
        ]
    }

    /// RFC 0199: the Lakehouse tab of the web console (EKOS RFC 0198).
    fn console_pages(&self) -> Vec<ekos::extension::ConsolePage> {
        console::pages()
    }

    /// RFC 0199: `ekos-lakehouse --version` names this binary, then the EKOS it is built on.
    fn binary(&self) -> Option<(&'static str, &'static str)> {
        Some(("ekos-lakehouse", env!("CARGO_PKG_VERSION")))
    }

    fn workflow_steps(
        &self,
        ctx: &ekos::extension::WorkflowContext<'_>,
    ) -> Vec<ekos_semantic::workflow::Step> {
        workflow::workflow_steps(ctx)
    }

    fn command_classes(&self) -> Vec<(&'static str, Class, bool)> {
        // Reaches services outside the EKOS home (RFC 0186): the source database among them.
        vec![
            ("lakehouse doctor", Class::External, false),
            // Both read the live source catalog.
            ("lakehouse plan", Class::External, false),
            ("lakehouse generate", Class::External, false),
            // Reads the source and bronze, writes facts to the project's ledger.
            ("lakehouse reconcile", Class::External, false),
            ("lakehouse run-report", Class::Write, false),
            // Reads the source, writes tombstones to bronze; `--accept` overrides the mass-delete
            // guard, a person's decision.
            ("lakehouse deletes", Class::Admin, false),
            ("lakehouse status", Class::Read, false),
            ("lakehouse tables", Class::Read, false),
            // RFC 0226: the state directory only. A change and an approval are a person's.
            ("lakehouse extraction show", Class::Read, false),
            ("lakehouse extraction change", Class::Review, false),
            ("lakehouse extraction approve", Class::Review, false),
            ("lakehouse install", Class::Local, false),
            ("lakehouse proposals", Class::Read, false),
            // Reach a webhook / a collector outside the home.
            ("lakehouse alert", Class::External, false),
            ("lakehouse metrics", Class::External, false),
            // Reads the live source; commits and branches in the pipeline repository.
            ("lakehouse sync", Class::External, false),
            // Empties a bronze table on its next run: its rows are loaded again from the source.
            ("lakehouse reload", Class::Admin, false),
            // Calls the configured (local) model for prose terms.
            ("lakehouse gold draft", Class::External, true),
            // Reads the repository and the ledger; writes metric facts.
            ("lakehouse check", Class::Write, false),
            // Reads the project's own source files (redacted), nothing else.
            ("lakehouse pentaho scan", Class::Read, false),
            // Reads the source files and the deployed plan, plans on Trino, branches the repository.
            ("lakehouse pentaho translate", Class::External, true),
            // Reads translation reports from the pipeline repository (files and branches).
            ("lakehouse pentaho report", Class::Read, false),
        ]
    }

    async fn run_command(
        &self,
        _name: &str,
        matches: &clap::ArgMatches,
        ctx: &CommandContext<'_>,
    ) -> anyhow::Result<()> {
        match matches.subcommand() {
            Some(("doctor", m)) => {
                let settings = settings::load(ctx.config)?;
                let mut checks = doctor::run(&settings).await;
                let redaction = ctx.config.redaction_config();
                checks.extend(off_runtime(|| Ok(doctor::deep(&settings, &redaction)))?);
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string(&checks)?);
                } else {
                    print!("{}", doctor::render(&checks));
                }
                doctor::verdict(&checks)
            }
            Some(("plan", m)) => {
                let settings = settings::load(ctx.config)?;
                let f = fresh(&settings, ctx, &pipeline_dir(&settings, ctx))?;
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string_pretty(&f.plan)?);
                } else {
                    print!("{}", plan_cmd::render_summary(&f.plan));
                    print_final(&f);
                }
                Ok(())
            }
            Some(("generate", m)) => {
                let settings = settings::load(ctx.config)?;
                workflow::gate(&settings, ctx)?;
                let out = match m.get_one::<std::path::PathBuf>("out") {
                    Some(o) => ctx.invoked_from.join(o),
                    None => pipeline_dir(&settings, ctx),
                };
                let mut silver = settings.silver_settings();
                let plan = if m.get_flag("from-lock") {
                    plan_cmd::read_lock(&out)?
                } else {
                    let f = fresh(&settings, ctx, &out)?;
                    print_final(&f);
                    plan_gate(&settings, ctx, &f)?;
                    silver.pii = f.masks.clone();
                    f.plan
                };
                let w = plan_cmd::write(&plan, &silver, &out)?;
                sync::commit_generated(&out, "lakehouse: generate")?;
                let n = w.files;
                print!("{}", plan_cmd::render_summary(&plan));
                print!("{}", plan_cmd::render_silver_report(&w.silver));
                for k in &w.kept {
                    println!("kept (edited by hand, not regenerated): {k}");
                }
                for r in &w.removed {
                    println!("removed (no longer in the plan): {r}");
                }
                println!("wrote {n} files under {}", out.display());
                Ok(())
            }
            Some(("deletes", m)) => deletes_cmd(ctx, m),
            Some(("extraction", m)) => extraction_cmd(ctx, m),
            Some(("reconcile", m)) => {
                let settings = settings::load(ctx.config)?;
                let plan = plan_cmd::read_lock(&pipeline_dir(&settings, ctx))?;
                let wanted: Vec<String> = m
                    .get_many::<String>("table")
                    .map(|v| v.cloned().collect())
                    .unwrap_or_default();
                let run = m.get_one::<String>("run-id").cloned().unwrap_or_else(|| {
                    let secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    format!("manual-{secs}")
                });
                let results = off_runtime(|| {
                    let src = plan_cmd::connect_source(&settings, &format!("lakehouse-{run}"))?;
                    let tgt = ekos_lakehouse_trino::TrinoReader::new(settings.trino_connection()?)?;
                    let mut out = Vec::new();
                    for t in &plan.tables {
                        let name = format!("{}.{}", t.schema, t.table);
                        if !wanted.is_empty() && !wanted.contains(&name) {
                            continue;
                        }
                        let r = reconcile::reconcile(t, &plan.settings, src.reader(), &tgt)?;
                        out.push((r, reconcile::evidence_queries(t, &plan.settings)));
                    }
                    Ok(out)
                })?;
                if results.is_empty() {
                    anyhow::bail!("no planned table matches {wanted:?}");
                }
                let store = facts::open_store_waiting(ctx.config, ctx.cwd)?;
                for (r, q) in &results {
                    facts::record_reconciliation(store.as_ref(), &run, r, q)?;
                }
                let reports: Vec<&reconcile::Reconciliation> =
                    results.iter().map(|(r, _)| r).collect();
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string(&reports)?);
                } else {
                    for r in &reports {
                        println!(
                            "{:<10} {:<36} control {}{}",
                            format!("{:?}", r.verdict).to_lowercase(),
                            r.table,
                            match r.control_fired {
                                Some(true) => "fired",
                                Some(false) => "MISSED",
                                None => "n/a (no rows)",
                            },
                            if r.not_compared.is_empty() {
                                String::new()
                            } else {
                                format!(", not compared: {}", r.not_compared.join(", "))
                            }
                        );
                    }
                }
                let bad: Vec<&str> = reports
                    .iter()
                    .filter(|r| r.verdict != reconcile::Verdict::Green)
                    .map(|r| r.table.as_str())
                    .collect();
                if !bad.is_empty() {
                    anyhow::bail!("not green: {}", bad.join(", "));
                }
                Ok(())
            }
            Some(("reload", m)) => {
                let settings = settings::load(ctx.config)?;
                let repo = pipeline_dir(&settings, ctx);
                let plan = plan_cmd::read_lock(&repo)?;
                let tables: Vec<String> = m
                    .get_many::<String>("table")
                    .map(|v| v.cloned().collect())
                    .unwrap_or_default();
                let cancel = m.get_flag("cancel");
                for t in sync::request_reload(&repo, &plan, &tables, cancel)? {
                    if cancel {
                        println!("reload withdrawn: {t}");
                    } else {
                        println!(
                            "reload requested: {t} — its next load empties bronze and loads every \
                             source row"
                        );
                    }
                }
                Ok(())
            }
            Some(("sync", m)) => {
                let settings = settings::load(ctx.config)?;
                workflow::gate(&settings, ctx)?;
                let repo = pipeline_dir(&settings, ctx);
                // RFC 0226: step 3 and the operator's changes apply; sync's own rules (RFC 0194)
                // decide what a source change does, so the plan approval does not gate it.
                let f = fresh(&settings, ctx, &repo)?;
                let mut silver = settings.silver_settings();
                silver.pii = f.masks.clone();
                let live = f.plan;
                let run = format!(
                    "sync-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                );
                let store = facts::open_store_waiting(ctx.config, ctx.cwd)?;
                let outcome = sync::sync(sync::SyncInputs {
                    repo: &repo,
                    live: &live,
                    silver: &silver,
                    policy: &settings.policy,
                    store: store.as_ref(),
                    git_remote: settings.git_remote.as_deref(),
                })?;
                for c in &outcome.applied {
                    facts::record_change(store.as_ref(), &run, c, None)?;
                }
                for p in &outcome.proposals {
                    for c in &p.changes {
                        facts::record_change(store.as_ref(), &run, c, Some(&p.branch))?;
                    }
                }
                for (table, (state, proposal)) in &outcome.status {
                    let state = format!("{state:?}").to_lowercase();
                    facts::record_schema_state(
                        store.as_ref(),
                        &run,
                        table,
                        &state,
                        proposal.as_deref(),
                    )?;
                }
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string(&outcome)?);
                } else {
                    for c in &outcome.applied {
                        println!(
                            "applied   {:<22} {}{}",
                            c.class.name(),
                            c.table,
                            c.column
                                .as_ref()
                                .map(|x| format!(".{x}"))
                                .unwrap_or_default()
                        );
                    }
                    for p in &outcome.proposals {
                        for c in &p.changes {
                            println!(
                                "proposed  {:<22} {}{}  → {}{}",
                                c.class.name(),
                                c.table,
                                c.column
                                    .as_ref()
                                    .map(|x| format!(".{x}"))
                                    .unwrap_or_default(),
                                p.branch,
                                if p.new { " (new)" } else { "" }
                            );
                        }
                        if let Some(pr) = &p.pull_request {
                            println!("          pull request: {pr}");
                        }
                    }
                    let held: Vec<String> = outcome
                        .status
                        .iter()
                        .filter(|(_, (s, _))| *s != sync::TableState::Active)
                        .map(|(t, (s, _))| format!("{t} {s:?}").to_lowercase())
                        .collect();
                    if outcome.applied.is_empty() && outcome.proposals.is_empty() {
                        println!("no changes: the source matches the deployed plan");
                    }
                    if !held.is_empty() {
                        println!("waiting on a proposal: {}", held.join(", "));
                    }
                }
                Ok(())
            }
            Some(("gold", g)) => {
                let Some(("draft", m)) = g.subcommand() else {
                    anyhow::bail!("unknown gold command");
                };
                let settings = settings::load(ctx.config)?;
                workflow::gate(&settings, ctx)?;
                let repo = pipeline_dir(&settings, ctx);
                let description = ctx.invoked_from.join(
                    m.get_one::<std::path::PathBuf>("description")
                        .expect("required"),
                );
                let (outcome, _) = gold_cmd::draft(
                    &repo,
                    &description,
                    settings.gold_model.as_deref().unwrap_or("llama3"),
                    settings
                        .ollama_url
                        .as_deref()
                        .unwrap_or("http://127.0.0.1:11434"),
                    &settings.trino_connection()?,
                )
                .await?;
                let run = format!(
                    "gold-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                );
                let store = facts::open_store_waiting(ctx.config, ctx.cwd)?;
                facts::record_mart(store.as_ref(), &run, &outcome)?;
                let questions_path = description.with_extension("questions.md");
                if outcome.questions.is_empty() {
                    let _ = std::fs::remove_file(&questions_path);
                    println!(
                        "proposed gold mart {} on {} ({} atoms checked, nothing invented)",
                        outcome.mart,
                        outcome.branch.as_deref().unwrap_or("?"),
                        outcome.checked_atoms
                    );
                    for (term, n) in &outcome.drafts {
                        println!("  {term}: drafted by the model in {n} attempt(s)");
                    }
                    Ok(())
                } else {
                    let spec = ekos_lakehouse_gold::parse(&std::fs::read_to_string(&description)?)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    std::fs::write(
                        &questions_path,
                        ekos_lakehouse_gold::questions_markdown(&spec, &outcome.questions),
                    )?;
                    for (term, q) in &outcome.questions {
                        println!("question about `{term}`: {}", q.text);
                    }
                    anyhow::bail!(
                        "{} question(s) for the author, written to {}",
                        outcome.questions.len(),
                        questions_path.display()
                    )
                }
            }
            Some(("pentaho", p)) => {
                let settings = settings::load(ctx.config)?;
                match p.subcommand() {
                    Some(("scan", m)) => pentaho_cmd::scan(&settings, ctx, m.get_flag("json")),
                    Some(("translate", m)) => {
                        let o = pentaho_cmd::TranslateOptions {
                            package: m.get_one::<String>("package").expect("required").clone(),
                            out: m
                                .get_one::<std::path::PathBuf>("out")
                                .map(|o| ctx.invoked_from.join(o)),
                            explain: !m.get_flag("no-explain"),
                            json: m.get_flag("json"),
                            allow_metered: m.get_flag("allow-metered"),
                        };
                        let repo = pipeline_dir(&settings, ctx);
                        pentaho_cmd::translate(&settings, ctx, &o, &repo).await
                    }
                    Some(("report", m)) => pentaho_cmd::report(
                        &settings,
                        ctx,
                        &pipeline_dir(&settings, ctx),
                        m.get_one::<String>("package").map(String::as_str),
                        m.get_one::<String>("step").map(String::as_str),
                        m.get_flag("json"),
                    ),
                    other => anyhow::bail!("unknown pentaho command {other:?}"),
                }
            }
            Some(("status", m)) => {
                let settings = settings::load(ctx.config)?;
                let store = ekos::commands::store::open_store_read_only(ctx.config, ctx.cwd)?;
                let h =
                    monitor::health(store.as_ref(), settings.late_after_minutes.unwrap_or(120))?;
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string(&h)?);
                } else {
                    print!("{}", monitor::render(&h));
                }
                Ok(())
            }
            Some(("install", m)) => {
                let home = ekos::home::resolve_home(
                    None,
                    std::env::var_os("EKOS_HOME").map(std::path::PathBuf::from),
                    ekos::home::user_home_from_env(),
                )?;
                if m.get_flag("uninstall") {
                    return off_runtime(|| install::uninstall(&home, m.get_flag("purge")));
                }
                let s = |k: &str| m.get_one::<String>(k).cloned();
                let o = install::Options {
                    home,
                    source: s("source"),
                    source_user: s("source-user").unwrap_or_default(),
                    source_password_env: s("source-password-env").unwrap_or_default(),
                    project: s("name"),
                    schemas: m
                        .get_many::<String>("schemas")
                        .map(|v| v.cloned().collect())
                        .unwrap_or_default(),
                    console: !m.get_flag("no-console"),
                    open: !m.get_flag("no-open"),
                    at_login: m.get_flag("at-login"),
                    first_run: !m.get_flag("no-first-run"),
                    console_port: *m.get_one::<u16>("console-port").expect("default"),
                };
                off_runtime(|| install::install(&o))
            }
            Some(("tables", m)) => {
                let settings = settings::load(ctx.config)?;
                let v = console::tables(&settings, &pipeline_dir(&settings, ctx));
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string(&v)?);
                } else {
                    println!("{}", serde_json::to_string_pretty(&v)?);
                }
                Ok(())
            }
            Some(("proposals", m)) => {
                let settings = settings::load(ctx.config)?;
                let v = console::proposals(&pipeline_dir(&settings, ctx));
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string(&v)?);
                } else {
                    for p in v["proposals"].as_array().into_iter().flatten() {
                        println!(
                            "{:<14} {:<30} {}  ({})",
                            p["kind"].as_str().unwrap_or(""),
                            p["subject"].as_str().unwrap_or(""),
                            p["summary"].as_str().unwrap_or(""),
                            p["branch"].as_str().unwrap_or("")
                        );
                    }
                    if v["proposals"].as_array().is_none_or(|a| a.is_empty()) {
                        println!("nothing waiting for a merge");
                    }
                }
                Ok(())
            }
            Some(("alert", _)) => {
                let settings = settings::load(ctx.config)?;
                let h = {
                    let store = ekos::commands::store::open_store_read_only(ctx.config, ctx.cwd)?;
                    monitor::health(store.as_ref(), settings.late_after_minutes.unwrap_or(120))?
                };
                let webhook = match &settings.alert_webhook_env {
                    Some(var) => Some(std::env::var(var).map_err(|_| {
                        anyhow::anyhow!("{var} names the alert webhook but is not set")
                    })?),
                    None => None,
                };
                let dir = pipeline_dir(&settings, ctx).join(".lakehouse");
                let sent = off_runtime(|| monitor::alert(&h, &dir, webhook.as_deref()))?;
                for t in &sent {
                    println!("{t}");
                }
                println!(
                    "{} transition(s){}",
                    sent.len(),
                    if webhook.is_some() {
                        " posted"
                    } else {
                        " (no alert-webhook-env: printed only)"
                    }
                );
                Ok(())
            }
            Some(("metrics", _)) => {
                let settings = settings::load(ctx.config)?;
                let endpoint = settings.otlp_endpoint.clone().ok_or_else(|| {
                    anyhow::anyhow!("set `otlp-endpoint` in [extensions.lakehouse]")
                })?;
                let h = {
                    let store = ekos::commands::store::open_store_read_only(ctx.config, ctx.cwd)?;
                    monitor::health(store.as_ref(), settings.late_after_minutes.unwrap_or(120))?
                };
                let body = monitor::otlp(&h, monitor::now());
                off_runtime(|| monitor::push_metrics(&endpoint, &body))?;
                println!("pushed {} table/layer series to {endpoint}", h.len());
                Ok(())
            }
            Some(("check", m)) => {
                let settings = settings::load(ctx.config)?;
                let repo = pipeline_dir(&settings, ctx);
                let plan = plan_cmd::read_lock(&repo)?;
                let silver = settings.silver_settings();
                let mut metrics = vec![];
                {
                    let store = ekos::commands::store::open_store_read_only(ctx.config, ctx.cwd)?;
                    metrics.push(check::lineage(&plan, &silver, store.as_ref())?);
                }
                metrics.extend(check::coverage(&plan, &silver, &repo)?);
                metrics.push(check::drift(&plan, &silver, &repo));
                if m.get_flag("lint") {
                    metrics.push(check::lint(&repo));
                }
                let run = format!(
                    "check-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                );
                let store = facts::open_store_waiting(ctx.config, ctx.cwd)?;
                for x in &metrics {
                    facts::record_check(store.as_ref(), &run, x)?;
                }
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string(&metrics)?);
                } else {
                    for x in &metrics {
                        println!(
                            "{:<4} {:<28} {}/{}",
                            if x.passed { "ok" } else { "FAIL" },
                            x.metric,
                            x.measured,
                            x.of
                        );
                        for g in x.gaps.iter().take(10) {
                            println!("       {g}");
                        }
                        if x.gaps.len() > 10 {
                            println!("       … {} more", x.gaps.len() - 10);
                        }
                    }
                }
                let failed: Vec<&str> = metrics
                    .iter()
                    .filter(|x| !x.passed)
                    .map(|x| x.metric)
                    .collect();
                if !failed.is_empty() {
                    anyhow::bail!("not met: {}", failed.join(", "));
                }
                Ok(())
            }
            Some(("run-report", m)) if m.get_one::<std::path::PathBuf>("dbt-results").is_some() => {
                let target = ctx.invoked_from.join(
                    m.get_one::<std::path::PathBuf>("dbt-results")
                        .expect("checked"),
                );
                let run = m.get_one::<String>("run-id").cloned().expect("required");
                let reports = monitor::dbt_results(&target)?;
                let store = facts::open_store_waiting(ctx.config, ctx.cwd)?;
                let failed = reports.iter().filter(|r| r.status != "ok").count();
                for mut r in reports.clone() {
                    r.run_id = run.clone();
                    facts::record_run(store.as_ref(), &r)?;
                }
                println!(
                    "recorded {} dbt model results ({failed} failing) for run {run}",
                    reports.len()
                );
                Ok(())
            }
            Some(("run-report", m)) => {
                let report = facts::RunReport {
                    layer: m.get_one::<String>("layer").cloned().expect("defaulted"),
                    table: m.get_one::<String>("table").cloned().expect("required"),
                    run_id: m.get_one::<String>("run-id").cloned().expect("required"),
                    status: m.get_one::<String>("status").cloned().expect("required"),
                    rows: m.get_one::<i64>("rows").copied(),
                    duration_ms: m.get_one::<u64>("duration-ms").copied(),
                    snapshot_id: m.get_one::<String>("snapshot-id").cloned(),
                    message: m.get_one::<String>("message").cloned(),
                };
                let store = facts::open_store_waiting(ctx.config, ctx.cwd)?;
                facts::record_run(store.as_ref(), &report)?;
                println!(
                    "recorded {} run {} ({})",
                    report.table, report.run_id, report.status
                );
                Ok(())
            }
            other => anyhow::bail!("unknown lakehouse command {other:?}"),
        }
    }
}

/// `lakehouse deletes` (RFC 0225 §2.4).
fn deletes_cmd(ctx: &CommandContext<'_>, m: &clap::ArgMatches) -> anyhow::Result<()> {
    use ekos_lakehouse_bronze::Cadence;
    let settings = settings::load(ctx.config)?;
    let plan = plan_cmd::read_lock(&pipeline_dir(&settings, ctx))?;
    let name = m.get_one::<String>("table").expect("required");
    let t = plan
        .tables
        .iter()
        .find(|t| &format!("{}.{}", t.schema, t.table) == name)
        .ok_or_else(|| anyhow::anyhow!("{name} is not in the deployed plan"))?;
    let Some(dp) = &t.decision.deletes else {
        anyhow::bail!(
            "{name} has no delete plan: it loads in full (deletes leave with the replacement) or \
             its generated deletes.sql finds them"
        );
    };
    let now = chrono::Utc::now();
    let run = m
        .get_one::<String>("run-id")
        .cloned()
        .unwrap_or_else(|| format!("manual-{}", now.timestamp()));
    let state_path = ctx.config.ekos_dir(ctx.cwd).join("lakehouse/deletes.json");
    let mut state = deletes::read_state(&state_path);
    let last = state.get(name).cloned();
    if let Cadence::Audit { days } = dp.cadence
        && !m.get_flag("force")
        && !deletes::due(last.as_deref(), days, now)
    {
        println!(
            "{name}: audit not due (last checked {}, every {days} days)",
            last.unwrap_or_default()
        );
        return Ok(());
    }
    // A retained purge leaves bronze's current state as `purge`; a propagated one as `delete`.
    let op = if dp.kind == "purge"
        && dp.purge_policy == Some(ekos_lakehouse_bronze::PurgePolicy::Retain)
    {
        "purge"
    } else {
        "delete"
    };
    let found = off_runtime(|| {
        let src = plan_cmd::connect_source(&settings, &format!("lakehouse-{run}"))?;
        let tgt = ekos_lakehouse_trino::TrinoReader::new(settings.trino_connection()?)?;
        deletes::find(t, &plan.settings, src.reader(), &tgt)
    })?;
    let mut check = deletes::Check {
        table: name.clone(),
        run: run.clone(),
        method: dp.method.clone(),
        kind: dp.kind.clone(),
        buckets: found.buckets,
        differing_buckets: found.differing.len(),
        source_keys_read: found.source_keys_read,
        bronze_keys_read: found.bronze_keys_read,
        bronze_keys: found.bronze_keys,
        found: found.deleted.len(),
        applied: 0,
        unloaded: found.unloaded,
        guard: None,
        contradicts_code: dp.contradicts_code && !found.deleted.is_empty(),
        last_seen: last,
        detected_at: now.to_rfc3339(),
    };
    if !m.get_flag("accept") {
        check.guard = deletes::guard(
            found.deleted.len(),
            found.bronze_keys,
            settings.mass_delete_percent.unwrap_or(5),
            settings.mass_delete_rows.unwrap_or(10_000),
        );
    }
    if check.guard.is_none() && !found.deleted.is_empty() {
        let admin = ekos_lakehouse_trino::Client::new(settings.trino_connection()?)?;
        for sql in deletes::tombstone_statements(t, &plan.settings, op, &found.deleted)? {
            for stmt in ekos_lakehouse_bronze::render::statements(
                &ekos_lakehouse_bronze::render::fill(&sql, &run, "", "NULL"),
            ) {
                admin.execute(&stmt)?;
            }
        }
        check.applied = found.deleted.len();
    }
    let store = facts::open_store_waiting(ctx.config, ctx.cwd)?;
    facts::record_delete_check(store.as_ref(), &check)?;
    println!(
        "{name}: {} of {} buckets differ; {} {}(s) found, {} applied as {op}; {} key(s) not loaded yet{}",
        check.differing_buckets,
        check.buckets,
        check.found,
        check.kind,
        check.applied,
        check.unloaded,
        if check.contradicts_code {
            " — the code says this table is never hard-deleted"
        } else {
            ""
        }
    );
    if let Some(why) = &check.guard {
        anyhow::bail!(
            "{name}: nothing applied, the deletes need the operator ({why}); when the cause is \
             known: lakehouse deletes --table {name} --accept"
        );
    }
    state.insert(name.clone(), check.detected_at.clone());
    deletes::write_state(&state_path, &state)
}

/// RFC 0226: the plan read fresh from the source with step 3 in and the deployed renames
/// carried, saved as the proposal, then with the operator's changes applied.
fn fresh(
    settings: &settings::Settings,
    ctx: &CommandContext<'_>,
    repo: &std::path::Path,
) -> anyhow::Result<extraction::Final> {
    let step3 = read_step3(ctx);
    let mut wf = facts::plan_facts(settings, ctx)?;
    step3.apply_facts(&mut wf);
    let mut plan = {
        let redaction = ctx.config.redaction_config();
        off_runtime(|| plan_cmd::build_plan_with(settings, &redaction, &wf))?
    };
    // Over a deployed plan: keep its approved renames, and rename in place what the source
    // renamed since, rather than add an empty column.
    if let Ok(deployed) = plan_cmd::read_lock(repo) {
        ekos_lakehouse_bronze::carry_renames(&mut plan, &deployed);
    }
    let excluded = step3.apply_plan(&mut plan);
    let masks = step3.masks_for(&plan);
    let p = extraction::Proposal {
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        plan,
        facts: wf,
        masks,
        excluded,
        step3: step3.subjects,
    };
    let dir = extraction::state_dir(ctx.config, ctx.cwd);
    extraction::write_proposal(&dir, &p)?;
    let changes = extraction::read_changes(&dir)?;
    Ok(extraction::finalize(&p, &changes, &settings.pii))
}

/// Step 3 from the review; without a ledger or a review, nothing (and the plan says why).
fn read_step3(ctx: &CommandContext<'_>) -> extraction::Step3 {
    let read = || -> anyhow::Result<extraction::Step3> {
        let store = ekos::commands::store::open_store_read_only(ctx.config, ctx.cwd)?;
        let r = ekos::commands::review::review_state(ctx.config, ctx.cwd, store.as_ref())?;
        Ok(extraction::step3(&r.statuses, &r.items))
    };
    read().unwrap_or_else(|e| extraction::Step3::unavailable(&format!("no review to read ({e})")))
}

fn print_final(f: &extraction::Final) {
    for a in &f.applied {
        match &a.stale {
            None => println!(
                "change applied: {} — {}",
                a.change.table,
                a.change.change.describe()
            ),
            Some(why) => println!(
                "change NOT applied: {} — {}: {why}",
                a.change.table,
                a.change.change.describe()
            ),
        }
    }
    if !f.excluded.is_empty() {
        println!("{} table(s) out of scope (step 3)", f.excluded.len());
    }
    for n in &f.notes {
        println!("note: {n}");
    }
}

/// RFC 0226 §2.5: under `require-plan-approval`, only the approved plan is built.
fn plan_gate(
    settings: &settings::Settings,
    ctx: &CommandContext<'_>,
    f: &extraction::Final,
) -> anyhow::Result<()> {
    if !settings.require_plan_approval.unwrap_or(false) {
        return Ok(());
    }
    let store = ekos::commands::store::open_store_read_only(ctx.config, ctx.cwd)?;
    let Some(o) = store.get_object(&facts::plan_approval_id())? else {
        anyhow::bail!(
            "require-plan-approval: the extraction plan is not approved. Review it (the console's \
             Extraction page, or `lakehouse extraction show`) and approve it"
        );
    };
    let approved = o
        .properties
        .get("digest")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if approved == f.digest {
        return Ok(());
    }
    let by = o
        .properties
        .get("by")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let at = o
        .properties
        .get("at")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let dir = extraction::state_dir(ctx.config, ctx.cwd);
    let diff = match extraction::read_approved(&dir)?.filter(|a| a.digest == approved) {
        Some(a) => extraction::differences(&a.tables, &f.tables),
        None => "the approved plan's tables are not recorded here".into(),
    };
    anyhow::bail!(
        "the plan differs from the one {by} approved at {at} ({diff}): the source, step 3 or a \
         change moved since. Review the plan and approve it again"
    )
}

fn reviewer(by: Option<&String>) -> anyhow::Result<String> {
    by.cloned()
        .or_else(|| std::env::var("USER").ok())
        .filter(|b| !b.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("who is deciding? Pass --as <you>"))
}

/// `lakehouse extraction show|change|approve` (RFC 0226).
fn extraction_cmd(ctx: &CommandContext<'_>, m: &clap::ArgMatches) -> anyhow::Result<()> {
    use ekos_lakehouse_bronze::extraction::{CadenceChoice, Change, ModeChoice, TableChange};
    let settings = settings::load(ctx.config)?;
    let dir = extraction::state_dir(ctx.config, ctx.cwd);
    let proposal = extraction::read_proposal(&dir)?;
    let changes = extraction::read_changes(&dir)?;
    let need = || {
        proposal
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no proposed plan yet: run `lakehouse plan` first"))
    };
    let now = || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    match m.subcommand() {
        Some(("show", m)) => {
            let f = proposal
                .as_ref()
                .map(|p| extraction::finalize(p, &changes, &settings.pii));
            let a = extraction::read_approved(&dir)?;
            let v = extraction::show(
                proposal.as_ref(),
                f.as_ref(),
                a.as_ref(),
                m.get_one::<String>("table").map(String::as_str),
            );
            if m.get_flag("json") {
                println!("{v}");
            } else {
                print!("{}", extraction::render(&v));
            }
            Ok(())
        }
        Some(("change", m)) => {
            let p = need()?;
            let wanted = m.get_one::<String>("table").cloned().unwrap_or_default();
            let names: Vec<String> = p
                .plan
                .tables
                .iter()
                .map(|t| format!("{}.{}", t.schema, t.table))
                .chain(changes.iter().map(|c| c.table.clone()))
                .collect();
            let exact: Vec<&String> = names
                .iter()
                .filter(|n| n.eq_ignore_ascii_case(&wanted))
                .collect();
            let by_name: std::collections::BTreeSet<&String> = names
                .iter()
                .filter(|n| {
                    n.rsplit_once('.')
                        .is_some_and(|(_, t)| t.eq_ignore_ascii_case(&wanted))
                })
                .collect();
            let table = match (exact.first(), by_name.len()) {
                (Some(t), _) => (*t).clone(),
                (None, 1) => (*by_name.iter().next().unwrap_or(&&wanted)).clone(),
                (None, 0) => anyhow::bail!("{wanted} is not in the proposed plan"),
                (None, _) => anyhow::bail!("{wanted} names several tables: give `schema.table`"),
            };
            let mut next = changes.clone();
            if m.get_flag("undo") {
                let before = next.len();
                next.retain(|c| c.table != table);
                extraction::write_changes(&dir, &next)?;
                println!("{table}: {} change(s) removed", before - next.len());
                return Ok(());
            }
            let change = if m.get_flag("full") {
                Change::Mode(ModeChoice::Full)
            } else if m.get_flag("incremental") {
                Change::Mode(ModeChoice::Incremental)
            } else if m.get_flag("deletes-every-run") {
                Change::Deletes(CadenceChoice::Run)
            } else if m.get_flag("deletes-audit") {
                Change::Deletes(CadenceChoice::Audit)
            } else if m.get_flag("keep-purged") {
                Change::Purge(ekos_lakehouse_bronze::PurgePolicy::Retain)
            } else {
                Change::Purge(ekos_lakehouse_bronze::PurgePolicy::Propagate)
            };
            let new = TableChange {
                table: table.clone(),
                change,
                reason: m
                    .get_one::<String>("reason")
                    .map(|r| r.trim().to_string())
                    .filter(|r| !r.is_empty()),
                by: reviewer(m.get_one::<String>("by"))?,
                at: now(),
            };
            ekos_lakehouse_bronze::extraction::record(&mut next, new.clone());
            let f = extraction::finalize(&p, &next, &settings.pii);
            if let Some(why) = f
                .applied
                .iter()
                .find(|a| a.change == new)
                .and_then(|a| a.stale.clone())
            {
                anyhow::bail!("{table}: cannot {}: {why}", new.change.describe());
            }
            extraction::write_changes(&dir, &next)?;
            let approved = extraction::read_approved(&dir)?;
            println!(
                "{table}: {} (by {}). {}",
                new.change.describe(),
                new.by,
                match approved {
                    Some(a) if a.digest == f.digest => "The plan is the approved one again.",
                    _ => "The plan needs approving before it is built.",
                }
            );
            Ok(())
        }
        Some(("approve", m)) => {
            let p = need()?;
            let f = extraction::finalize(&p, &changes, &settings.pii);
            let by = reviewer(m.get_one::<String>("by"))?;
            let a = extraction::approve(
                &p,
                &f,
                &changes,
                &by,
                &now(),
                m.get_one::<String>("note").cloned(),
            )?;
            let store = facts::open_store_waiting(ctx.config, ctx.cwd)?;
            facts::record_plan_approval(store.as_ref(), &a)?;
            extraction::write_approved(&dir, &a)?;
            let c = &a.counts;
            println!(
                "extraction plan approved by {by}: digest {}\n  incremental {} (deletes every \
                 run {}, audited {}, other {}), full {}, refused {}, out of scope {}, masked \
                 columns {}, changes {}",
                &a.digest[..16],
                c.incremental_every_run + c.incremental_audited + c.incremental_other,
                c.incremental_every_run,
                c.incremental_audited,
                c.incremental_other,
                c.full,
                c.refused,
                c.excluded,
                c.masked_columns,
                a.changes.len()
            );
            Ok(())
        }
        _ => anyhow::bail!("lakehouse extraction: show, change or approve"),
    }
}

/// The pipeline repository: `--out`, else `pipeline-dir` relative to the project.
fn pipeline_dir(s: &settings::Settings, ctx: &CommandContext<'_>) -> std::path::PathBuf {
    settings::pipeline_base(ctx.cwd, ctx.config_path)
        .join(s.pipeline_dir.as_deref().unwrap_or("pipeline"))
}

/// Run blocking database work on its own thread, off the async runtime (the clients block).
fn off_runtime<T: Send>(f: impl FnOnce() -> anyhow::Result<T> + Send) -> anyhow::Result<T> {
    std::thread::scope(|s| {
        s.spawn(f)
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the worker thread panicked")))
    })
}

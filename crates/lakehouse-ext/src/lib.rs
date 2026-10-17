//! EKOS Lakehouse as an EKOS extension (RFC 0190, seam: RFC 0149 / 0191).
//!
//! EKOS stays in the control plane: it reads the source, generates pipeline code into a Git
//! repository the user owns and records every run and check as ledger facts. Trino moves the
//! data and the orchestrator schedules it; nothing here ever moves a row.

pub mod check;
pub mod doctor;
pub mod facts;
pub mod plan_cmd;
pub mod reconcile;
pub mod settings;
pub mod source;
pub mod sync;

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
                                .required(true)
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
                                .required(true)
                                .value_parser(["ok", "failed"])
                                .help("How the load ended"),
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
            // Reads the live source; commits and branches in the pipeline repository.
            ("lakehouse sync", Class::External, false),
            // Reads the repository and the ledger; writes metric facts.
            ("lakehouse check", Class::Write, false),
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
                let checks = doctor::run(&settings).await;
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string(&checks)?);
                } else {
                    print!("{}", doctor::render(&checks));
                }
                doctor::verdict(&checks)
            }
            Some(("plan", m)) => {
                let settings = settings::load(ctx.config)?;
                let plan = {
                    let redaction = ctx.config.redaction_config();
                    off_runtime(|| plan_cmd::build_plan(&settings, &redaction))?
                };
                if m.get_flag("json") {
                    println!("{}", serde_json::to_string_pretty(&plan)?);
                } else {
                    print!("{}", plan_cmd::render_summary(&plan));
                }
                Ok(())
            }
            Some(("generate", m)) => {
                let settings = settings::load(ctx.config)?;
                let out = match m.get_one::<std::path::PathBuf>("out") {
                    Some(o) => ctx.invoked_from.join(o),
                    None => pipeline_dir(&settings, ctx),
                };
                let plan = {
                    let redaction = ctx.config.redaction_config();
                    off_runtime(|| plan_cmd::build_plan(&settings, &redaction))?
                };
                let w = plan_cmd::write(&plan, &settings.silver_settings(), &out)?;
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
                        let r = reconcile::reconcile(t, &plan.settings, &src, &tgt)?;
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
            Some(("sync", m)) => {
                let settings = settings::load(ctx.config)?;
                let repo = pipeline_dir(&settings, ctx);
                let live = {
                    let redaction = ctx.config.redaction_config();
                    off_runtime(|| plan_cmd::build_plan(&settings, &redaction))?
                };
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
                    silver: &settings.silver_settings(),
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
            Some(("run-report", m)) => {
                let report = facts::RunReport {
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

/// The pipeline repository: `--out`, else `pipeline-dir` relative to the project.
fn pipeline_dir(s: &settings::Settings, ctx: &CommandContext<'_>) -> std::path::PathBuf {
    ctx.cwd
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

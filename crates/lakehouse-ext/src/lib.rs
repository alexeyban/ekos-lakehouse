//! EKOS Lakehouse as an EKOS extension (RFC 0190, seam: RFC 0149 / 0191).
//!
//! EKOS stays in the control plane: it reads the source, generates pipeline code into a Git
//! repository the user owns and records every run and check as ledger facts. Trino moves the
//! data and the orchestrator schedules it; nothing here ever moves a row.

pub mod doctor;
pub mod facts;
pub mod plan_cmd;
pub mod reconcile;
pub mod settings;
pub mod source;

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
                let plan = off_runtime(|| plan_cmd::build_plan(&settings))?;
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
                let plan = off_runtime(|| plan_cmd::build_plan(&settings))?;
                let n = plan_cmd::write(&plan, &out)?;
                print!("{}", plan_cmd::render_summary(&plan));
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

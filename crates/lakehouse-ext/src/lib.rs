//! EKOS Lakehouse as an EKOS extension (RFC 0190, seam: RFC 0149 / 0191).
//!
//! EKOS stays in the control plane: it reads the source, generates pipeline code into a Git
//! repository the user owns and records every run and check as ledger facts. Trino moves the
//! data and the orchestrator schedules it; nothing here ever moves a row.

pub mod doctor;
pub mod settings;

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
        vec![("lakehouse doctor", Class::External, false)]
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
            other => anyhow::bail!("unknown lakehouse command {other:?}"),
        }
    }
}

# EKOS Lakehouse

Generated, maintained medallion pipelines from a PostgreSQL database into Apache Iceberg tables.
Trino is the engine, Polaris the REST catalog and any S3 store the storage (SeaweedFS in the
sandbox); transforms are dbt-trino and
the orchestrator is Dagster.

EKOS Lakehouse is an extension of [EKOS](https://github.com/alexeyban/EKOS), the compiler for
enterprise knowledge. EKOS reads the source, compiles it into evidence-backed knowledge, and
from that knowledge this extension generates pipeline code into a Git repository you own. It
never moves data itself: Trino moves rows, Dagster schedules runs, and people approve what is
lossy, breaking or business-facing.

**Status: all six milestones of RFC 0190's v1 plan are built (M0–M5).** The design is
[RFC 0190](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0190-lakehouse.md), bronze is
[RFC 0192](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0192-lakehouse-bronze.md).
A PostgreSQL source lands in Iceberg bronze tables on a schedule, every table is reconciled
against its source on every run, and a generated dbt-trino project builds tested silver tables
([RFC 0193](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0193-lakehouse-silver.md)).
Source schema changes are classified and either applied or proposed as branches with their
impact ([RFC 0194](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0194-lakehouse-schema-changes.md)).
Gold marts are drafted from written descriptions, with every term resolved or asked back and
nothing invented ([RFC 0195](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0195-lakehouse-gold.md)).
Health per table and layer, alerts, OpenTelemetry metrics and MCP tools for agents
([RFC 0196](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0196-lakehouse-monitoring.md)).
Pentaho (Kettle) packages are translated into silver and gold dbt models, every step labelled
exact, rewritten, llm-drafted or gap, JavaScript and Java steps included, proposed on a branch
([RFC 0200](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0200-pentaho-to-lakehouse-dbt.md);
[docs/pentaho.md](docs/pentaho.md)).

## Install

On Linux x86_64 with Docker:

```bash
curl -fsSL https://raw.githubusercontent.com/alexeyban/ekos-lakehouse/main/install.sh | sh
```

One command installs the program, starts the stack, creates a sample project, runs its pipeline
once and opens the web console on its **Lakehouse** tab. For your own database add
`-s -- --source host:5432/db --source-user reader` (password in `EKOS_LH_SOURCE_PASSWORD`).
**[docs/install.md](docs/install.md)** says what it changes and how to remove it;
**[docs/console.md](docs/console.md)** describes the tab.

To do each step yourself instead: **[docs/quickstart.md](docs/quickstart.md)** — your PostgreSQL
database to monitored Iceberg tables. Then the [runbook](docs/runbook.md) and
[troubleshooting](docs/troubleshooting.md).

![The Lakehouse tab of the EKOS console](docs/console.png)

## Build

From source (any platform Rust and Docker run on; a release package is Linux x86_64 only). The
`[patch]` block in `Cargo.toml` builds against a sibling EKOS checkout when it is uncommented:

```bash
git clone https://github.com/alexeyban/EKOS ../EKOS
cargo build
./target/debug/ekos-lakehouse --help
```

The binary is `ekos-lakehouse`, not `ekos`, so it never shadows a plain EKOS install. Every EKOS
command works from it; the Lakehouse commands are under `ekos-lakehouse lakehouse …`.

## Configure

Settings live in the project's `ekos.toml`, in their own table. Credentials never go here.

```toml
[extensions.lakehouse]
trino-url          = "http://127.0.0.1:58080"
polaris-health-url = "http://127.0.0.1:58182/q/health"
s3-url             = "http://127.0.0.1:58333"
dagster-url        = "http://127.0.0.1:53000"
source             = "127.0.0.1:55433"
```

```toml
source-database     = "pagila"
source-user         = "ekos"
source-password-env = "EKOS_LH_PG_PASSWORD"     # the variable's name, never the password
source-catalog      = "pagila"                  # the Trino catalog that reads the source
bronze-schedule     = "0 * * * *"

[extensions.lakehouse.watermarks]
"public.film" = "last_update"     # used only when a BEFORE UPDATE trigger proves it
```

```bash
ekos-lakehouse lakehouse doctor          # which services answer; --json for a program
ekos-lakehouse lakehouse plan            # each table's mode, mappings and refusals
ekos-lakehouse lakehouse generate        # SQL + lakehouse.lock.json + Dagster project → pipeline/
ekos-lakehouse lakehouse reconcile       # V1–V3 with a planted control, recorded in the ledger
ekos-lakehouse lakehouse check --lint    # lineage, test/doc coverage, drift, sqlfluff
ekos-lakehouse lakehouse sync            # source changes: safe ones committed, the rest proposed
ekos-lakehouse lakehouse reload --table public.rental   # next load empties bronze and loads every row
ekos-lakehouse lakehouse gold draft marts/revenue.md   # a mart from a description, or questions
ekos-lakehouse lakehouse pentaho scan    # Kettle packages: transformations, jobs, steps, links
ekos-lakehouse lakehouse pentaho translate etl   # silver + gold dbt models on a proposal branch
ekos-lakehouse lakehouse pentaho report --package etl   # per model and step: fidelity, gaps, SQL
```

A gold mart starts as a committed description:

```markdown
---
mart: receivables_by_customer_month
grain: [customer, month]
dimensions:
  customer: entity_credit_account.meta_number
  month: month of transactions.transdate
measures:
  invoiced: sum of ar.amount_bc
  average_invoice: the amount_bc total divided by the number of invoices
filters:
  - transactions.approved is true
---
What each customer was invoiced per month.
```

Every term must resolve to a silver column, or it comes back as a question. Prose terms are
drafted by a local model (`gold-model`, Ollama) and must state nothing the description does not
and run on Trino. The mart is proposed on a branch with grain and reconciliation tests.

`sync` commits a new table, a new nullable column and a widened type on the pipeline
repository's branch; a rename, a drop, a narrowed type, a new required column, a key change or
a changed watermark trigger becomes a branch `ekos/change/<table>/<id>` with
`proposals/<id>.md` (the change, its evidence, the silver columns and tests it reaches).
Merging the branch is the approval; a merged rename renames the bronze column in place, so the
rows already loaded keep their values. With `git-remote = "origin"` the branch is pushed and a
pull request opened with `gh` (signed in, with rights on the repository); `sync` then starts by
fast-forwarding from the remote, so a request merged there resumes its table on the next run,
pushes the safe changes it commits, and removes merged proposal branches from the remote.
`[extensions.lakehouse.policy]` changes a class's action.

Masking and history are opt-in, by a person:

```toml
[extensions.lakehouse]
history = ["public.customer"]            # SCD2 snapshot in silver

[extensions.lakehouse.pii]
"public.customer.email" = "hmac"         # key from EKOS_LH_PII_KEY
"public.staff.password" = "null"
```

The source's Trino catalog needs `postgresql.array-mapping=AS_ARRAY` and
`unsupported-type-handling=CONVERT_TO_VARCHAR`: with the defaults the connector silently drops
columns of types it does not map. A table whose column the connector does not expose is refused.

## Try it on the sandbox

```bash
docker compose -f deploy/docker-compose.lakehouse.yml up -d      # everything but Dagster
deploy/fixtures/pagila.sh                                         # Pagila, pinned
# in a project directory with the ekos.toml above:
ekos-lakehouse init && ekos-lakehouse lakehouse generate
EKOS_LH_PROJECT=$PWD EKOS_LAKEHOUSE_BIN=$(which ekos-lakehouse) \
  docker compose -f <repo>/deploy/docker-compose.lakehouse.yml --profile orchestrator up -d lh-dagster
# Dagster at http://127.0.0.1:53000 — the bronze job runs on its schedule
```

## Roadmap

| Milestone | Delivers |
|---|---|
| M0 | the stack in one compose file, Trino SQL in EKOS, cross-engine checksum agreement, load throughput measured |
| M1 | bronze: load mode per table from profile facts, Iceberg DDL and maintenance, reconciliation every run |
| M2 | silver: dbt-trino models generated from source facts, DQ rules as tests, the project compiled back into EKOS |
| M3 | schema changes: classified, safe ones applied, the rest as pull requests with their impact |
| M4 | gold: marts from written descriptions, every term resolved or asked back, anti-invention at 100% |
| M5 | monitoring, alerts, OpenTelemetry, MCP read tools, self-serve docs |

## Licence

MIT, as EKOS.

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

**Status: early development (milestone M0 of six).** The design is
[RFC 0190](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0190-lakehouse.md). Today the
binary contains all of EKOS plus one command, `lakehouse doctor`. Nothing generates pipelines yet.

## Build

This repository builds against a sibling EKOS checkout while it needs EKOS changes that are not on
EKOS `main` yet (see the `[patch]` block in `Cargo.toml`):

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

```bash
ekos-lakehouse lakehouse doctor          # which services answer; --json for a program
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

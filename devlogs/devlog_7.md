# Devlog 7 — M5 exit met: monitoring, alerts, metrics, MCP, and a quickstart that is a test

**Date:** 2026-10-17
**PRs:** none (local)
**Branch:** main
**Design:** EKOS RFC 0196 (`ekos/docs/rfcs/0196-lakehouse-monitoring.md`)

---

## Summary

Health per table and layer — healthy, late, unverified, failing, schema pending — computed from
the ledger alone, so the CLI, the alert webhook, OpenTelemetry metrics and two MCP tools for agents
all say the same thing. `doctor` now checks what fails later and quietly. And the quickstart is
executed verbatim by `deploy/e2e/quickstart.sh`: a fresh database to monitored bronze and silver,
4/4 healthy — the RFC 0190 v1 exit. Running it found a real false green.

---

## What was built

| Piece | What |
|---|---|
| `crates/health` (pure, 4 tests) | facts → one state per (table, layer) by precedence, with evidence and a probable cause; transitions for alerts |
| ledger facts | `sync` records each table's schema state (fixed id: the current version is the current state); `run-report --dbt-results` records each silver/gold model's build and tests (via `manifest.json`) |
| `lakehouse status` / `alert` / `metrics` | the grid; each transition once to a webhook (`alert-webhook-env`); OTLP/HTTP JSON to a collector (`otlp-endpoint`) |
| MCP `ekos_pipeline_status`, `ekos_table_health` | through the ledger only; marked scoped (RFC 0191) |
| `doctor` (deep) | every source column exposed by the connector; source role read-only (warning otherwise); Iceberg catalog; gold model pulled |
| Dagster | dbt results recorded after every build; `lakehouse/health` asset runs `alert` and `metrics` |
| `deploy` | optional OTel collector 0.161.0 (`--profile observability`): OTLP in, Prometheus out; `git` in the Dagster image |
| docs | `quickstart.md` (executed), `runbook.md`, `troubleshooting.md` |

## Verified

| Check | Result |
|---|---|
| Pagila after 15 h without Dagster | 16 × `late`, each with the run that proves it |
| MCP over stdio | both tools listed; `ekos_table_health` returned film's runs and reconciliations; a restricted reader may call them and sees no tables (Lakehouse facts carry no source) |
| alerts to a local webhook | 16 transitions posted, then 0 on the next run |
| metrics through the collector | Prometheus at `:58889/metrics`: `lakehouse_table_state` = 0 for 33 series after a full cycle |
| a full Dagster cycle on Pagila | sync + 16 bronze + dbt + health, 102 s, 35 materializations; 33 healthy |
| `doctor` on LedgerSMB | all 1 034 columns exposed; role can write → WARN (the sandbox superuser — true) |
| `deploy/e2e/quickstart.sh` | 7 doc blocks run verbatim: build, stack, a `shop` database, its catalog, the project, generate, one run → 4/4 healthy |

---

## Knowledge Captured

- **`dagster-dbt` gives each invocation its own target directory.** Reading `dbt/target/`
  recorded an *older* run's results — the first Pagila cycle's "healthy silver" came from my
  manual dbt builds. Found only because the quickstart started from an empty directory. Read
  `invocation.target_path`, and fail the asset when recording fails.
- **`prepare_if_dev` only prepares under `dagster dev`**: a one-off `dagster job execute` had no
  manifest. EKOS regenerates the dbt project, so the manifest is prepared on every load.
- **`git` was missing from the Dagster image** and `sync` died with a bare "No such file or
  directory": the error now says what is missing.
- **MCP tools only get the ledger** (no config, no files), which is why health had to be ledger-only.
- **A docs page run as a test** finds what component tests cannot: the missing `.dagster`
  directory, the manifest, the stale results.

## Decided against, for now

The console tab (the console has no extension mechanism; Grafana, CLI and MCP cover it).

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/health/` | new crate |
| `crates/lakehouse-ext/src/{monitor,doctor,facts,settings,lib,sync}.rs` | monitoring, MCP tools, deep doctor |
| `crates/bronze/src/dagster.py` | dbt results, health asset, manifest preparation |
| `deploy/{otel/,dagster/Dockerfile,docker-compose.lakehouse.yml}` | collector, git |
| `deploy/e2e/quickstart.sh`, `docs/` | the executed quickstart, runbook, troubleshooting |

# Devlog 4 — M2 exit met: silver for Pagila and LedgerSMB, tested, linted, lineage-complete

**Date:** 2026-10-17
**PRs:** none (local)
**Branch:** main
**Design:** EKOS RFC 0193 (`ekos/docs/rfcs/0193-lakehouse-silver.md`)

---

## Summary

Silver is a dbt-trino project generated from the bronze plan's source facts: one model per
bronze table (deduplicated to bronze's current state, approved masks applied), tests from the
catalog, docs from source comments, declared history as YAML snapshots. `lakehouse check`
compiles the project back through EKOS's own dbt analyzer and measures lineage, coverage,
drift and lint. Both fixtures pass every gate; the Dagster project now builds bronze and
silver in one scheduled job with dbt tests as asset checks.

---

## Results

| | Pagila | LedgerSMB |
|---|---|---|
| tables planned / refused | 16 / 0 | 167 / 1 (`tax`: ±infinity) |
| dbt build | 131/131 (16 models, 114 tests, 1 snapshot) | 167 models, all tests pass (in the Dagster run) |
| Dagster run (bronze + silver) | 81 s scheduled, 16 + 17 assets, 114 checks | 852 s, 167 + 167 assets, 0 failed |
| lineage through the ledger | 107/107 (3 masked excluded) | 1193/1193 |
| key / not-null / relationship tests | 16/16 · 78/78 · 19/19 | 157/157 · 580/580 · 291/291 |
| documentation | 0/110 (Pagila has no comments) | 237/1193 |
| lint (sqlfluff 4.3.0, trino, core rules) | 0 errors | 169/169 files clean |
| drift | 82/82 | 679/679 |

LedgerSMB came from EKOS Migrate's sandbox (`pg_dump` → the lakehouse source, PG 16 → 18).

---

## What was built

| Piece | What |
|---|---|
| `crates/silver` (pure) | models, model YAML with tests + `meta.ekos` evidence + Dagster asset keys, sources YAML keyed to bronze assets, YAML snapshots, `generate_schema_name`, `ekos_unique_combination` test, `.sqlfluff`, `.gitignore` |
| source facts | foreign keys, table/column comments (redacted, RFC 0043), enum labels, PII name candidates, ±infinity counts; lock format 2 |
| `lakehouse check` | lineage (ledger), coverage (repository YAML), drift (manifest + regeneration), lint (`--lint`); each a `LakehouseMetric` fact |
| `generate` manifest | `.generated.json`: edited files kept and reported, dropped tables' files removed, foreign files refused |
| Dagster | `dagster-dbt` assets after bronze; one job, 4 at a time; the PII key in the service env |

---

## Knowledge Captured

- **Trino's PostgreSQL connector reads `'infinity'` as a finite date** (year -171979) with no
  error; it loads. Measured at plan time and refused; reconciliation reports it as a divergence.
- **A value one side cannot render canonically used to stop `reconcile`** ("cannot parse '' as
  u32" — a NULL bucket); it is now a tier divergence, and the planted control is skipped once a
  tier has diverged.
- **Sources declare the same foreign key twice** (LedgerSMB); dbt refuses two tests of one name.
- **A stale generated model broke the whole Dagster code location**: `dagster-dbt` runs
  `dbt parse` at load, and one model whose source was gone failed every asset's loading.
- **sqlfluff's jinja templater has no `env_var` and cannot parse `{% snapshot %}`**: the PII key
  is a project var; snapshots are dbt ≥ 1.9 YAML.
- **An empty-string default for the HMAC key would hash with no key**: the var has no default.
- **One failed bronze asset skips the whole dbt step** (it depends on all of bronze); a refused
  table must leave the plan, not fail at load.
- **The machine runs out of memory with the stack + Dagster + a desktop**: Claude Code reaped a
  background wait during the LedgerSMB run (the run itself finished). Stop Dagster between runs.
- The PII name classifier flags `address_id` as a postal address: candidates are reported only.

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/silver/` | new crate, 5 tests |
| `crates/bronze/src/{model,plan,typemap}.rs` | facts for silver, ±infinity refusal |
| `crates/lakehouse-ext/src/{source,check,facts,plan_cmd,reconcile,settings,lib}.rs` | facts, `check`, manifest, tolerant reconcile |
| `crates/lakehouse-ext/tests/generate_files.rs` | keep / remove / refuse rules |
| `crates/bronze/src/dagster.py` | silver assets |
| `deploy/` | sqlfluff in the image, PII key env, `ledgersmb` catalog |

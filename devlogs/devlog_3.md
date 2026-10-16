# Devlog 3 — M1 exit met: Pagila lands in bronze on a schedule, reconciled every run

**Date:** 2026-10-16
**PRs:** none (local)
**Branch:** main
**Design:** EKOS RFC 0192 (`ekos/docs/rfcs/0192-lakehouse-bronze.md`)

---

## Summary

Bronze is generated from PostgreSQL's own catalog, checked against the Trino connector, and
landed in Iceberg by a generated Dagster project. Every run reports to EKOS and reconciles each
table at V1–V3 with a planted control; a table that is not green fails its asset. Exit: Pagila
(16 tables, PostgreSQL 18 + pgvector) loaded on a 5-minute schedule twice in a row, all 16
green with the control firing, the incremental tables appending only what changed.

---

## What was built

| Piece | What |
|---|---|
| `crates/bronze` (pure) | type map with lossiness classes; load-mode choice with evidence; SQL renderers (load, create, watermark, deletes, current state, maintenance, namespace); the plan / `lakehouse.lock.json`; the Dagster template |
| `lakehouse-ext/src/source.rs` | PostgreSQL catalog (tables, types with kind/domain base/array element, keys, BEFORE UPDATE triggers and what they assign) + the connector's columns; unconstrained numeric measured |
| `lakehouse-ext/src/reconcile.rs` | V1–V3 against bronze's current state; the planted control; verdict green / diverged / unverified |
| `lakehouse-ext/src/facts.rs` | `LakehouseRun`, `LakehouseMetric` (with evidence queries) in the project ledger; waits for the write lock |
| commands | `lakehouse plan`, `generate`, `reconcile`, `run-report` |
| `deploy/fixtures/pagila.sh` | Pagila pinned to 9baf49c, SHA-256 checked |
| `lh-dagster` | host network, host user, runs the generated project; `--profile orchestrator` |

---

## Measured before designing (Pagila on the sandbox)

- **Trino's PostgreSQL connector silently drops columns** under its defaults: the domain
  `year`, `text[]`, `tsvector` — 3 of 94 columns, no error. With
  `postgresql.array-mapping=AS_ARRAY` and `unsupported-type-handling=CONVERT_TO_VARCHAR` all
  appear. The generator reads PostgreSQL's catalog and refuses a table whose column the
  connector does not expose.
- The connector lists views and every partition as tables (55 `payment_p…`).
- Iceberg rejects `smallint` (widened to integer) and the `number` an unconstrained numeric
  becomes (measured precision/scale → `decimal(38,s)`, else refused).

## Results

| Check | Result |
|---|---|
| `lakehouse plan` on Pagila | 16 tables, 0 refused; film and rental incremental (trigger-proven), payment's declared `payment_date` refused as a watermark (no trigger assigns it) |
| live test: load all, reconcile | 16/16 green, control fired on every table; `tsvector` as text equals PostgreSQL's text byte for byte |
| live test: update + insert, then delete on film | green both times; exactly one tombstone |
| `lakehouse reconcile` after the test reverted a value | **diverged** on film (V2 `last_update:max`, one V3 bucket) — bronze was stale, as it was |
| Dagster, 5-minute schedule | 1st tick failed (two real bugs below), fixed; 17:00 and 17:05 runs SUCCESS, 22 s and 18 s; film's 2nd run appended 1 row |

---

## Knowledge Captured

- **`CREATE SCHEMA IF NOT EXISTS` is not race-safe against an Iceberg REST catalog**: parallel
  assets lose with "Namespace already exists". The asset treats that answer as success.
- **EKOS's fact ledger takes one writer at a time**; parallel `reconcile` processes collided on
  the write lock. Ledger writes now wait (backoff, 2 min) — the reconciliation itself runs before
  the store is opened, so the lock is held only for the append.
- **A bind-mounted binary keeps the old inode**: after `cargo build` the container still runs the
  previous binary until restarted.
- **`>=` on the watermark re-reads ties** (one row per run on Pagila); `>` would skip rows
  committed later with the same timestamp. Silver dedups.
- **Pagila's current schema needs PostgreSQL 18 and pgvector** (`uuidv7()`, a `vector(20)`
  column): the sandbox source is now `pgvector/pgvector:0.8.7-pg18-trixie`, whose data lives
  under `/var/lib/postgresql/18/` (mount the parent).
- EKL has no boolean literal (`control_fired = true` is a parse error).

## Not done in M1

- `lakehouse doctor` does not yet read the source catalog's two connector settings (the plan's
  refusal is the safety net today).
- Partitioning of bronze tables (all unpartitioned).

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/bronze/` | new crate |
| `crates/lakehouse-ext/src/{source,reconcile,facts,plan_cmd,settings,lib}.rs` | sources, reconcile, facts, commands |
| `crates/lakehouse-ext/tests/bronze_live.rs` | Pagila end to end |
| `deploy/` | PostgreSQL 18 + pgvector, Pagila loader, `pagila` catalog, Dagster service |

# Devlog 5 — M3 exit met: source schema changes, all nine classes

**Date:** 2026-10-17
**PRs:** none (local)
**Branch:** main
**Design:** EKOS RFC 0194 (`ekos/docs/rfcs/0194-lakehouse-schema-changes.md`)

---

## Summary

`lakehouse sync` compares the live source with the deployed plan by PostgreSQL's own
identities (table OID, column `attnum`), classifies every change into RFC 0190's nine classes,
commits the safe ones and proposes the rest on branches a person merges — with the impact from
the compiled ledger. The live test drove all nine classes with real DDL; each produced its
documented action, and merging every proposal brought every table back to green.

---

## What was built

| Piece | What |
|---|---|
| `crates/changes` (pure, 11 tests) | `diff` by OID/attnum, nine classes, policy, `apply_auto`, `with_table`, `change_id`, proposal Markdown |
| lock format 3 | table OID, column attnum and default, watermark function body SHA-256 |
| bronze renderers | `INSERT … (columns)` everywhere; `schema.sql` (idempotent ADD COLUMN / SET DATA TYPE) for incremental tables |
| `lakehouse sync` | live plan → diff → safe changes committed (author `ekos-lakehouse`) → one proposal branch per table in a worktree → `.lakehouse/status.json` → `LakehouseChange` facts; `git-remote` + `gh` for pull requests |
| proposal branches | rebuilt when HEAD moves, deleted when superseded or no longer needed |
| `generate` | initialises the pipeline repository as git and commits |
| Dagster | `lakehouse/sync` asset before every load; paused tables skipped with their proposal; `schema.sql` before incremental loads |

## The exit test (`tests/changes_live.rs`, 25 s)

| Round | Source DDL | Action checked |
|---|---|---|
| 1 | new table; new nullable column; `integer → bigint` | applied and committed in one commit; incremental table promoted by `schema.sql`; loads green (values above 2³¹ included) |
| 2 | NOT NULL column with default; rename; drop; `numeric(10,2) → (10,4)`; new primary key; trigger function body changed | six proposals; rename/drop/narrowing **paused** (and the deployed load really fails on the renamed source); the other three **pending**, still loading green |
| 3 | a reviewer merges each branch | open proposals rebuilt on the new HEAD and merge cleanly; afterwards no changes, all active, loads green |

On Pagila with its compiled ledger, renaming `film.title` proposed it with "silver
`public__film.title` reads it — tests: not_null"; renaming the masked `customer.email` with
"silver `public__customer.email` is computed from it". Both renames were reverted; `sync` then
removed both branches.

---

## Knowledge Captured

- **Trino/Iceberg no-op DDL writes no metadata**: `ADD COLUMN IF NOT EXISTS` on an existing
  column and `SET DATA TYPE` to the current type leave `$metadata_log_entries` unchanged. Iceberg
  promotes int→bigint, real→double, decimal precision up; refuses narrowing.
- **Proposals regenerate shared files**, so they conflict once one is merged; rebuilding EKOS's
  own branches on HEAD (dependency-bot style) keeps every open one mergeable.
- **A key column's rename changed the mode's evidence text**, which first read as a routine
  change: compare the watermark proof alone.
- **A masked column is invisible to lineage** (computed), so impact needs the model's columns too.
- **A positional `INSERT … SELECT`** breaks the moment bronze and source differ in columns;
  every generated insert names its columns now.

## Not done

- Pull requests on GitHub: implemented (`git-remote`), not exercised — there is no repository yet.
- Dagster reloads its code location only on restart, so a newly added table's asset appears then.
- V5 for routine changes (bronze reads tables; the routine that matters is a watermark trigger).

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/changes/` | new crate |
| `crates/bronze/src/{model,plan,mode,render,dagster.py}` | identities, column lists, `schema.sql`, `.gitignore`, sync/pause in Dagster |
| `crates/lakehouse-ext/src/{sync,facts,settings,lib,reconcile}.rs` | `sync`, `LakehouseChange`, policy, git remote |
| `crates/lakehouse-ext/tests/changes_live.rs` | the nine-class exit test |
| `deploy/trino/catalog/m3.properties` | the test's scratch source |

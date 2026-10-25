# Devlog 15 — Incremental loads from write behaviour, and deletes without a marker (EKOS RFC 0225)

**Date:** 2026-10-25
**PRs:** none (`main`, pushed)
**Branch:** main
**Design:** EKOS RFC 0225; EKOS devlog_301

---

## Summary

`application-watermark = "UPDATED"` lets a table load incrementally where EKOS's write-behaviour facts say the
application sets the column on every write, and gives each such table a delete plan. `lakehouse deletes` finds hard
deletes by a bucketed key diff, stops at the mass-delete guard, writes tombstones (`delete`, or `purge` for a retained
purge) and records counts only. On iDempiere 739 of 901 tables become incremental; the live suite drives a single
delete, a delete + re-insert, a 148-key mass delete (guard, then accepted) and a retained purge, each green after.

---

## What was built

| Component | Change |
|---|---|
| `bronze/src/model.rs` | `WriteFacts` |
| `bronze/src/mode.rs` | `choose_with`: trigger proof, then the application watermark; `DeletePlan`, `Cadence`, `PurgePolicy`; new `ModeSettings` keys serialized only when set |
| `bronze/src/plan.rs` | `plan_with(sources, settings, facts)` |
| `bronze/src/render.rs` | `tombstones_where` (shared with `deletes_sql`, whose text is unchanged), `retains_purges`, current state excludes `purge` for those tables |
| `silver/src/lib.rs` | a retained purge keeps its row's last real version in silver |
| `bronze/src/dagster.py` | `lakehouse deletes` after an incremental load with a delete plan |
| `lakehouse-ext/src/deletes.rs` | `find` (buckets, keys in differing buckets), `guard`, `tombstone_statements`, state file, `due` |
| `lakehouse-ext/src/lib.rs` | `lakehouse deletes --table [--run-id] [--accept] [--force]` (admin class) |
| `lakehouse-ext/src/facts.rs` | `write_facts`, `plan_facts`, `record_delete_check` |
| `lakehouse-ext/src/settings.rs` | `application-watermark`, `incremental-min-rows`, `delete-audit-days`, `purge-policy`, `mass-delete-percent`, `mass-delete-rows`; `pipeline_base` |
| `tests/oracle_live.rs` | `idempiere_deletes_are_found_guarded_and_tombstoned` |

## Knowledge Captured

- **A home project's pipeline was written into its sources.** `pipeline-dir` resolved against the workspace; for an
  EKOS home project that is the user's checkout. Now it resolves beside the project's `ekos.toml` unless the workspace
  carries the installer's `.lakehouse/project.json` (the installer's projects keep the old layout).
- A mass delete stopped by the guard leaves bronze's rows in place: reconciliation diverges until it is accepted, so a
  pending table never reads as green.
- Reading the facts is a full pass over the ledger (19 s plan on iDempiere): only done when `application-watermark`
  is set.

## Files Changed

See the table above; plus `crates/lakehouse-ext/Cargo.toml` (`chrono`), `README.md`.

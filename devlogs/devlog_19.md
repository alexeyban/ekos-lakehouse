# Devlog 19 — Step 7: marts proposed, merged, built and reconciled (EKOS RFC 0228)

**Date:** 2026-10-26
**PRs:** none (`main`)
**Branch:** main
**Design:** EKOS RFC 0228; EKOS devlog_305

---

## Summary

`lakehouse marts show|propose|merge|build|reconcile` and the console's Marts page take each approved requirement
(step 4) to a reconciled gold table. Merge is the person's approval and a ledger fact; Build runs only the mart
through Dagster; Reconcile reads each measure's total in the mart and over silver. On iDempiere four marts went from
approved to reconciled with every total equal, the fourth on the page (browser 8/8).

---

## What was built

| Component | Change |
|---|---|
| `gold/src/lib.rs` | `reconcile_sql`, `fidelity`, `lineage` (`Lineage` type) |
| `lakehouse-ext/src/marts.rs` | states, git reads (byte-exact `file_at`), state files, show/render, the bar's step |
| `lakehouse-ext/src/lib.rs` | the `marts` commands and classes; `gold draft`'s body as `gold_draft()`; `run --asset`; `run-report` writes gold builds |
| `run_cmd.rs` | asset lookup by name, `assetSelection`, code-location reload |
| `facts.rs` | `record_mart_merge`, `record_mart_reconciliation` |
| `workflow.rs` | step 7 |
| `console.rs` | the Marts page |

## Decisions

- Build runs in-process (one mart; no process per step).
- A reconciliation only counts for the build it checked (`run_id`); a new build makes the mart `built` again.
- The merged branch is deleted, so `proposed` never lingers after a merge.

## Knowledge Captured

- `sync::git` trims stdout: never digest a file read through it.
- Generated schedules default to RUNNING; starting Dagster on a test machine past the hour starts the full job.
- `reloadRepositoryLocation` is needed for an asset added after Dagster loaded the code.
- dagster-dbt gold keys are `["gold", "gold__<mart>"]`.

## Files Changed

| File | Change summary |
|---|---|
| `crates/gold/src/{lib,tests}.rs` | reconciliation, fidelity, lineage |
| `crates/lakehouse-ext/src/{marts,lib,run_cmd,facts,workflow,console}.rs` | step 7 |
| `README.md` | the marts section |

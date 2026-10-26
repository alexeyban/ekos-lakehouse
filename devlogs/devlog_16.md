# Devlog 16 — The extraction plan, reviewed and approved (EKOS RFC 0226)

**Date:** 2026-10-26
**PRs:** none (`main`, pushed)
**Branch:** main
**Design:** EKOS RFC 0226; EKOS devlog_302

---

## Summary

The plan now carries step 3's signed decisions (scope, PII masks, write-behaviour overrides), is saved as a proposal,
and can be changed per table and approved — from the CLI (`lakehouse extraction show|change|approve`) or the console's
new Extraction page. `require-plan-approval = true` makes `generate` build only the approved plan. On the iDempiere
testbed: 849 tables planned, 53 out of scope, one column masked; the gate refused a build after a change was undone
and after a column was added in Oracle, naming the table each time.

---

## What was built

| Component | Change |
|---|---|
| `bronze/src/extraction.rs` | per-table changes and their rules; `orderable` is now public |
| `lakehouse-ext/src/extraction.rs` | step 3 in, proposal, finalize, digest, approval, step 5, the page's JSON |
| `lakehouse-ext/src/lib.rs` | `fresh()` for `plan`/`generate`/`sync`; `plan_gate`; the `extraction` commands (show: read; change, approve: review) |
| `lakehouse-ext/src/facts.rs` | `record_plan_approval` (`LakehouseApproval`, one id per project) |
| `lakehouse-ext/src/workflow.rs` | step 5 from the extraction state instead of "tables synced" |
| `lakehouse-ext/src/console.rs` | the Extraction page (needs EKOS `Action::inputs`) |
| `settings.rs` | `require-plan-approval` |

## Decisions

- `sync` applies step 3 and the changes but is not gated by the approval: source changes are its job (RFC 0194).
- The digest leaves evidence text out (row estimates move); everything a build reads is in it.
- After a mode change the proposal's evidence moves to the notes as "the proposal said: …", so the page does not give
  the old mode's reasons for the new one.

## Knowledge Captured

- `plan` must carry the deployed lock's renames like `generate`, or the proposal's digest never matches the build.
- A change that alters nothing ("load in full" on a full table) left the plan approved and looked like a bug; it is
  refused now.
- Pagila (no review): lock and SQL unchanged.

## Files Changed

| File | Change summary |
|---|---|
| `crates/bronze/src/{extraction,lib,mode}.rs` | changes |
| `crates/lakehouse-ext/src/{extraction,lib,facts,workflow,console,settings}.rs` | step 5 |
| `README.md` | the extraction plan section |
| `Cargo.lock` | EKOS at the `Action::inputs` commit |

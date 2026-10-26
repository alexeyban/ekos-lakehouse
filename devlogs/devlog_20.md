# Devlog 20 — Stage lines, step 2 from the estate record, the demo book (EKOS RFC 0218)

**Date:** 2026-10-26
**PRs:** none (direct to `main`); EKOS devlog_306 (`1d6c090`)
**Branch:** `main`

---

## Summary

EKOS now writes a run log for every command that changes something; this repository adds the lines worth reading
to its step 4–7 commands, reads step 2 from the record `ekos docs generate --layout estate` leaves, and ships
`tools/demo-book`: a Playwright tour of the console and a renderer that turns what it captured into a book (Markdown
pages + PDF). The iDempiere book is in the demo repository.

---

## What was built

| Component | Change |
|---|---|
| `lib.rs` | `run_log::emit` lines: gold `drafted` (branch, atoms, drafted terms, questions); marts `merged`, `build launched`/`built` (Dagster run id), `reconciled` (measure, both totals, equal); requirements `resolved`/`approved`; extraction `changed`, `changes undone`, `approved` (digest, counts) |
| `workflow.rs` | `ESTATE_RECORD`, `docs_step`: step 2 *todo* without a record, *partial* with one; `steps()` takes it |
| `tools/demo-book` | `steps.py` (pages, commands, narration drafts), `tour.py` (screenshots, facts, runs; `--act`, `--replace`), `render.py`, `pdf.py`, tests |
| `Cargo.lock` | EKOS `1d6c090` |

## Decisions
- A line carries counts, names and aggregate totals, never a row value.
- The book reuses lakehouse-docs' manual sections and page format, so its publishers publish it unchanged.
- `--act` only runs what leaves the project as it was; approvals, sign-offs, merges and builds are never clicked.

## Knowledge Captured
- An action with no inputs and no confirmation runs on the click (no Run button).
- A table with a change appears in two panels of the Extraction page; only the tables panel selects it.
- Chromium cannot screenshot the 849-table Lakehouse page at full height; the tour photographs the window.

## Files Changed

| File | Change summary |
|---|---|
| `crates/lakehouse-ext/src/lib.rs` | stage lines |
| `crates/lakehouse-ext/src/workflow.rs` | step 2 from the estate record; test |
| `tools/demo-book/` | new |
| `README.md` | run logs and the demo book |

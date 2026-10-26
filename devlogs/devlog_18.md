# Devlog 18 — Step 4: mart requirements, drafted, resolved, approved (EKOS RFC 0215)

**Date:** 2026-10-26
**PRs:** none (`main`)
**Branch:** main
**Design:** EKOS RFC 0215; EKOS devlog_304

---

## Summary

`lakehouse requirements draft|edit|show|approve` and the console's Requirements page. A mart is described in plain
words, and the model routed to EKOS's new `requirements` task drafts its header from a catalogue excerpt of the plan.
The gold resolver then decides every term: names in any case, `… via table.column` to choose among join paths,
paths of at most four keys, one path per table. Questions go to the operator. Approval commits `marts/<mart>.md` and
records `LakehouseApproval` `mart:<name>`; under `require-plan-approval`, `gold draft` builds only approved
requirements. On iDempiere the plan's example request went from a `llama3` draft through nine edits to an approved
requirement and a gold mart with nothing invented; the browser run passed 14/14.

---

## What was built

| Component | Change |
|---|---|
| `gold/src/lib.rs` | case-insensitive names, `via`, bounded and target-directed path search, one path per table, similar-name candidates, `= true` |
| `gold/src/requirements.rs` | `render`, `apply` (term / drop term / filters), `rank`, `choose`, `excerpt`, `from_answer` |
| `lakehouse-ext/src/requirements.rs` | the state directory, draft from text or file, edits with a log, show, approval, `require_approved` |
| `lakehouse-ext/src/lib.rs` | the `requirements` commands and classes; the gate in `gold draft` |
| `facts.rs` | `record_requirement_approval` |
| `workflow.rs` | step 4 from the requirements' states |
| `console.rs` | the Requirements page |

## Decisions

- The model drafts once per request (up to three attempts for an unreadable answer). It is never asked to resolve a
  question.
- `via` filters the enumerated paths; questions name, per path, its last key the other paths do not all share.
- A draft lives in the state directory; only an approved requirement reaches the pipeline repository.

## Knowledge Captured

- From `C_ORDERLINE`, 36 key paths reach `C_REGION` within four hops; pruning the walk by the reverse distance to the
  target makes the search instant.
- llama3 copies placeholders and borrows example names; a concrete example in another domain works better than a
  format with `TABLE.COLUMN`.
- Console: external commands need the command's name typed; the console caches classes until it restarts; only
  review-class commands get the signed-in person as `--as`.

## Files Changed

| File | Change summary |
|---|---|
| `crates/gold/src/{lib,requirements,tests}.rs` | resolver and requirements |
| `crates/lakehouse-ext/src/{requirements,lib,facts,workflow,console}.rs` | step 4 |
| `README.md` | the requirements section |
| `Cargo.lock` | EKOS at `b945e07` (`Task::Requirements`) |

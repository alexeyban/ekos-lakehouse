# Devlog 31 — Talend: second run, verification in the console, chained keyed replace, LLM fill-in (EKOS RFC 0268)

**Date:** 2026-11-22
**PRs:** none (direct to `main`); EKOS devlog_355
**Branch:** `main`

---

## Summary

The rest of RFC 0268:
- **The differential run compares a second, incremental run.** Fixture B's original ODS jobs can't load a change:
  their closing update violates their own `NOT NULL` columns and rolls back, though the job exits 0. The verification
  says so, with the job's error line.
- **The console's Talend tab shows the verification.**
- **Fixture E's delete-then-insert jobs with the target joined inside a chain are keyed replaces:** 19, up from 10.
- **Gaps can be drafted by a model through the router.** Ollama needs no flag, anything else `--allow-metered`.

---

## What was built

| Part | What |
|---|---|
| `fixture-b.json` `runs` | Run 2's edits (replace a text, append a line), made on the host on a copy of the inputs |
| `run.sh` / `vm-run.ps1` | `project-r<k>` inputs, every job run again on the tables run 1 left, dumps `r1_…` / `r2_…`, jobs tagged with their run |
| `lakehouse talend verify` | Restructured into steps (`land`, `build_all`, `compare`). Run 2: re-land, then each model's incremental build into `__r2` tables. A table or view is rebuilt; an append adds the incremental rows; a merge replaces the rows by key; a snapshot closes a changed version and adds new versions. `{{ this }}` is run 1's table. Each compared model carries `job_errors`. |
| `talend report` | `verification` attached; `*.verification.json` and `*.drafts.json` never read as translations |
| Console | "Verified by execution" panel |
| `patterns::keyed_replace` | The left join on the target anywhere upstream of both outputs. The insert's path rebuilt on the staged side when no node on it reads a target column (`reads_columns` per node kind; a pass-through of a target column dropped). A delete with no filter accepted (a NULL key deletes nothing). |
| `etl-dbt::draft` | `DraftRequest`, `Answer`, `parse_answer`, `check`, moved from `pentaho-dbt` and re-exported there; Pentaho golden identical |
| `talend-rel` | `Options.drafts` (no longer `Copy`), `Lowering.draftable`. A gap with one known input is offered; a draft replaces it (`Filter` for `keep_when`, then a `Project` of `Expr::Sql`, `llm-drafted`). |
| `talend-dbt::draft` | Talend's prompt (each input field's SQL name and its Java spelling, the translator's reason marked as its own), requests, `key_of` |
| `talend translate --allow-metered` | Drafting through `draft_all_with`, now taking a prompt function; drafts kept in `<pkg>.drafts.json` and used by `verify` |
| `tests/talend_draft_live.rs` | Gated (`EKOS_OLLAMA_MODEL`): the real router and checks on the fixtures |

## Results

| | |
|---|---|
| Run 2 | `DIM_CALENDRIER` equal. `ODS_CATEGORIE` and `ODS_TYPE_CLIENT` differ: the original jobs fail on their own schema, the snapshot does not. |
| Fixture E | 19 keyed replaces, 27 / 146 enabled; `FACT_MOV_COST` refused (it deletes a computed key) |
| Drafting, local | `glm-4.7-flash`: 2 of C's 5 gaps passed the checks. `qwen2.5-coder:3b`: 0 of 7. |
| C–E sandbox | not verified by execution: no built jobs in the fixtures |

---

## Knowledge Captured

- **A Talend job that exits 0 may have rolled back its batch.** Its error output holds the refused statement, and
  the runner keeps it.
- **PowerShell variables are case-insensitive:** `$run` is `$Run`.
- **Trino's memory catalog has no `MERGE`.** An incremental run is emulated by building new tables.
- **A small model copies whatever names it is shown into SQL.** The prompt marks which spelling is SQL.

---

## Files Changed

| File | Change summary |
|---|---|
| `scripts/talend-sandbox/{run.sh,vm-run.ps1,fixture-b.json}` | runs |
| `crates/lakehouse-ext/src/{talend_cmd,pentaho_cmd,console,lib}.rs` | run 2, `job_errors`, report, panel, drafting |
| `crates/lakehouse-ext/tests/talend_draft_live.rs` | new, gated |
| `crates/etl-dbt/src/draft.rs` | new (moved) |
| `crates/pentaho-dbt/src/draft.rs` | re-exports |
| `crates/talend-dbt/src/{draft,patterns,verify,lib}.rs` | drafting, chained keyed replace, `job_errors`, settings |
| `crates/talend-rel/src/{lower,lib}.rs`, `tests/phase2.rs` | drafts offered and applied |
| `crates/talend-dbt/tests/fixtures.rs` | E: 19 keyed replaces |
| `Cargo.toml` files | `etl-rel` in the extension; `serde` in `etl-dbt` |

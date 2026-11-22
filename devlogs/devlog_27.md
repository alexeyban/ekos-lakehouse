# Devlog 27 — Talend jobs to dbt: `etl-dbt`, `talend-dbt` and `lakehouse talend translate` (EKOS RFC 0268, phase 1)

**Date:** 2026-11-22
**PRs:** none (direct to `main`); EKOS devlog_351
**Branch:** `main`

---

## Summary

The last step of the Talend translation path: lowered Talend jobs (RFC 0267) become a dbt project for the
lakehouse. The new pieces:
- the engine-neutral half of `pentaho-dbt` moved to a new crate, `etl-dbt` (RFC 0266's method: re-exports, golden
  manifests identical);
- `talend-dbt` writes silver and gold models, with a rule per data action, four gates and a reviewer's report;
- `lakehouse talend translate` / `talend scan`.

On the fixtures, no model is disabled by a gate, and `dbt parse` accepts the generated projects. Many models stay
disabled, each for a stated reason: mostly the two patterns phase 2 is for (two writers of one table, and a job
reading what it writes).

| Fixture | Silver enabled | Gold enabled | Questions |
|---|---|---|---|
| B | 9 / 30 | 1 / 43 | 74 |
| C | 34 / 39 | 8 / 22 | 38 |
| D | 9 / 18 | 0 / 5 | 14 |
| E | 12 / 116 | 5 / 68 | 186 |

(Plan synthetic: every PostgreSQL table a job reads and no job writes.)

---

## What was built

| Part | What |
|---|---|
| `crates/etl-dbt` (new) | Moved from `pentaho-dbt`: `catalog` (`Resolver`), `pgsql` (PostgreSQL → Trino), `slug`, `engine_sql`, `parses_as_trino`, `plannable_sql`. `pentaho-dbt` re-exports them, and `neutral.rs` now covers this crate too. |
| `crates/talend-dbt` (new) | `translate(project, plan, settings)`. Silver: the chain from each source. Gold: one per sink, materialized by `Rule` (`reload`, `append`, `merge`, `update`, `no-key`, `delete`, `file`). Across jobs, a table written by one is a `ref()` in the others. Statements and watermarks are reported, and key tests go into `_<job>.yml`. Output: `translation.json` and `proposals/talend-<project>.md`. |
| `talend-dbt::gates` | Java left in the SQL (`raw_java`); anti-invention (`invented`: every identifier is a node's or a table's column); `inert` (Jinja markers broken in project text); `sources_as_tables`. |
| Gates in `translate` | Trino parse; disabling of every model on a `ref()` cycle, and of every reader of a disabled model, until none is left |
| `talend-rel` | `TableRef.connection` from the component or its `tDBConnection`: engine, `DBNAME`, `SCHEMA_DB`. A file input that a `tFileList` names carries `$path`, and a tMap resolves it to the main input's copy. |
| `lakehouse-ext` `talend_cmd` | `talend scan` (`read`); `talend translate <project> [--out] [--no-explain]` (`external`): redacted read, the deployed plan, `EXPLAIN` on Trino, then a proposal branch `ekos/talend/<project>/<id>` or `--out`. `write_translation_of(tool, …)`. `generate` leaves `talend/` paths alone. |

## Implementation details worth remembering

- **dbt renders Jinja in comments and YAML descriptions.** A disabled model's commented SQL kept its `{{ ref() }}`,
  so it still made a dependency. A folded Talend file name holding `{{(String)globalMap.get(…)}}` broke
  `dbt parse`. Every text from the project, and commented SQL, goes through `gates::inert`.
- **dbt refuses an enabled model that reads a disabled one.** The translator disables such readers until none is
  left. `dbt parse` found it; no Rust test had.
- **`project` is a global EKOS argument.** An extension subcommand's positional named `project` is read as the home
  project, so it is `talend-project`.
- **The anti-invention gate found a lowering bug on its first run.** `talend-rel` lowered `tFileList`'s
  `CURRENT_FILEPATH` to `$path` without carrying `$path` from the source.
- **An ELT input names its table, not its database.** A database-less reference matches the one table the project
  writes under the same schema and name.

## Decisions

- **Gold models are named per component (`talend__<project>__<job>__<component>`), not per table.** Talend often
  writes one table from two components, and phase 1 disables the second writer with a question rather than
  guessing they are one load.
- **Files are `talend_files` sources, not seeds.** Where a file is landed is the operator's decision, so it is
  asked.
- **`dbt parse` is not part of `cargo test`.** The test suite does not depend on a dbt install. It was run by hand
  on B and E, and the result is recorded here and in RFC 0268.

---

## Knowledge Captured

- **A cycle of `ref()`s is the Talend "lookup the target" pattern** (left-join the table you are loading to find
  existing rows), and a dbt model cannot read itself. The phase 2 forms are snapshots and `{{ this }}` incremental
  models.
- **Running `dbt parse` without a warehouse.** Any installed adapter works with a placeholder profile, because
  parse never connects: here `databricks`, host `example.invalid`. Stub every `ref()` that names a model of the
  lakehouse's own repository.

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/etl-dbt/*` | new crate (moved code) |
| `crates/pentaho-dbt/src/{lib,catalog,pgsql}.rs`, `Cargo.toml` | re-exports |
| `crates/etl-rel/tests/neutral.rs` | covers `etl-dbt` |
| `crates/talend-dbt/*` | new crate; tests `translate.rs` (synthetic), `fixtures.rs` (B–E), `pure.rs` |
| `crates/talend-rel/src/lower.rs` | connections on table refs; `$path` |
| `crates/lakehouse-ext/src/{talend_cmd,pentaho_cmd,plan_cmd,lib}.rs`, `Cargo.toml` | the commands |
| `Cargo.toml`, `Cargo.lock` | new members |

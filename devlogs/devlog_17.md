# Devlog 17 — Silver from the source model, and iDempiere through Dagster (EKOS RFC 0227)

**Date:** 2026-10-26
**PRs:** none (`main`)
**Branch:** main
**Design:** EKOS RFC 0227; EKOS devlog_303

---

## Summary

The plan now carries Oracle check lists and the application model's code facts per column. Silver turns `Y`/`N`
flags into booleans, tests declared lists strictly and code lists and logical keys as warnings, flags retained purges,
and closes deleted rows in history. `lakehouse run` launches the Dagster job. A closed 165-table iDempiere subset ran
in one Dagster process: bronze all green, 165 silver models, 3,664 of 3,667 tests passing, 3 warnings on real data.
The run found six bugs, all fixed. The full 849-table run is still open: it needs more memory than this machine has.

---

## What was built

| Component | Change |
|---|---|
| `bronze/src/model.rs`, `plan.rs` | `ValueList`, `CodeFacts`, `ColumnFacts.check`/`.code`, `attach_checks`/`attach_code`, `safe_name`, `iceberg_name` |
| `bronze/src/typemap_oracle.rs` | `CHAR(n>1)` → `rpad(CAST(c AS varchar), n, ' ')`; `CHAR(1)` plain; unknown length refused |
| `bronze/src/render.rs` | Iceberg target named by `iceberg_name` |
| `bronze/src/dagster.py` | `_safe`/`_iceberg_name`; no WAL query on Oracle; dbt manifest prepared only when stale |
| `silver/src/lib.rs` | flags, check/code-list/code-reference tests, bronze source flag tests, purge flag, snapshot hard deletes, `flags-as-text`, `typed_values_arg`, YAML 1.1 quoting, `$`-free aliases |
| `lakehouse-ext/src/source.rs`, `source_oracle.rs` | `value_lists` (check constraints `c IN (…)`) |
| `lakehouse-ext/src/facts.rs` | `ledger_facts`: write behaviour + `ModelTable` code facts in one pass |
| `lakehouse-ext/src/run_cmd.rs` | `lakehouse run [--job] [--location] [--in-process] [--wait]` over Dagster GraphQL |
| `lakehouse-ext/src/sync.rs` | `expression_changes`: reloads for incremental tables whose load expression changed |
| `lakehouse-ext/src/plan_cmd.rs` | the lock is never kept as a hand edit |
| `pentaho-dbt/src/lib.rs` | silver references use the Iceberg name |
| `console.rs` | **Run the pipeline** |

## Decisions

- Declared facts win: a column with a check constraint is not tested on the code's list; a declared foreign key
  suppresses the code-reference test.
- A code-only flag is tested on bronze, since after the conversion an unexpected value is already NULL.
- `iceberg_name` changes only names with `$` (the Trino problem); `safe_name` (Dagster/dbt) also replaces `#` and the
  rest, but renaming every unusual Iceberg table would move existing tables.
- A changed expression asks for a reload rather than rewriting rows in place: the reload path already exists and is
  reconciled (RFC 0194).

## Knowledge Captured

- Trino's `char(n)` → `varchar` cast drops trailing spaces; Oracle keeps them.
- Trino reads `t$x` as a metadata table; Polaris refuses a replace whose new location is not the table's.
- PyYAML reads bare `ON`/`off`/`yes` as booleans; serde_yaml leaves them bare.
- dagster-dbt runs tests eagerly for a model subset; `dbt test --indirect-selection cautious` for a subset.
- The sandbox Trino (2 GiB) is OOM-killed by 4 dbt threads over thousands of tests; 2 threads are fine.
- `dagster asset materialize -c inproc.yaml` (`execution: config: in_process: {}`) runs a selection in one process:
  ~1 GB, against 3.6 GB for `dagster dev` with a process per step.

## Files Changed

| File | Change summary |
|---|---|
| `crates/bronze/src/{model,plan,lib,render,typemap_oracle}.rs`, `dagster.py` | facts, names, CHAR padding, template |
| `crates/silver/src/lib.rs` | silver from the source model |
| `crates/lakehouse-ext/src/{source,source_oracle,facts,plan_cmd,lib,run_cmd,sync,settings,console}.rs` | facts, run, reloads |
| `crates/pentaho-dbt/src/lib.rs` | Iceberg names |
| `README.md` | the silver section |

# Devlog 22 — Each project's own silver and gold schemas; column lineage for upper-case names (EKOS RFC 0231)

**Date:** 2026-10-27
**PRs:** none (direct to `main`); EKOS devlog_310
**Branch:** `main`

---

## Summary

Two defects found while recording the end-to-end video demo, a second project (`idemsales`, prefix `bronze_sales_`)
on the iDempiere testbed. Silver and gold schema names ignored the project's namespace prefix, so the new project's
silver rebuilt the first project's 33 tables and its marts would have overwritten the first project's gold. Now a
project's **layer tag** (its prefix without the default `bronze_` lead) goes into both: `silver_sales_adempiere`,
`gold_sales`; the default prefix generates exactly what it did before. Separately, lakehouse-docs' column lineage
lost every Oracle column: upstream keys were matched with their case, and Oracle names are upper case.

---

## What was built

| Component | Change |
|---|---|
| `bronze::plan` | `layer_tag(prefix)`, `gold_schema(prefix)` |
| `silver` | `silver_schema(t)` takes the tag from the table's namespace |
| `gold` | the model config's `schema` is the project's gold schema; `Compiled.tag` for `engine_sql` (EXPLAIN, reconciliation) |
| `lakehouse-ext` | `marts reconcile` reads the project's gold schema; `marts::GOLD_SCHEMA` removed |
| `tools/lakehouse-docs` | `lineage.py`: case-insensitive upstream lookup, the column name mapped back; `pages.py` text names the tag |

## Implementation details worth remembering
- The tag keeps its trailing `_` for silver (`silver_` + `sales_` + `adempiere`) and drops it for gold (`gold_sales`).
- A prefix without the `bronze_` lead is its own tag: `raw_` → `silver_raw_public`, `gold_raw`.
- On regeneration only the 33 silver models' `schema=` line changed; nothing else in the pipeline moved.

## Decisions
- **No migration of old tables.** A project with a custom prefix gets new schemas on its next `generate`; the old
  tables stay until an operator drops them. Dropping from a generator would be a silent destructive act.
- **Pentaho translation untouched** (`silver_pentaho` / `gold`): a separate flow, out of RFC 0231.

## Knowledge Captured
- Two lakehouse projects on one catalog collide in every layer the prefix does not reach. Check silver and gold
  schema names when adding a second project, not only bronze.
- Oracle reports identifiers in upper case and dbt's SQL refers to them in lower case: any name lookup across the
  two must be case-insensitive and map back to the declared name for display.

## Files Changed

| File | Change summary |
|---|---|
| `crates/bronze/src/plan.rs` | `layer_tag`, `gold_schema` + tests |
| `crates/silver/src/lib.rs` | tagged silver schema + test |
| `crates/gold/src/{lib,tests}.rs` | gold schema in config, `Compiled.tag` + tests |
| `crates/lakehouse-ext/src/{lib,marts}.rs` | reconcile reads the project's gold schema |
| `tools/lakehouse-docs/lakehouse_docs/{lineage,pages}.py`, `tests/test_generate.py` | case-insensitive lineage + test |

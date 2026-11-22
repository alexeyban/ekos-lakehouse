# Devlog 26 — `talend-rel` phase 2: ELT maps, statements, `tJavaRow`, row order, digest routines (EKOS RFC 0267)

**Date:** 2026-11-22
**PRs:** none (direct to `main`); EKOS devlog_350
**Branch:** `main`

---

## Summary

Phase 2 of the Talend lowering. Fixture E goes from 73% to 100% of its data components:
- its 21 ELT maps are now read through sqlparser;
- its 42 `tSnowflakeRow` statements become a new shared node, `Op::Statement`;
- its 14 watermark `tJavaRow`s are lowered.

The 12 queries that read a watermark are no longer marked `exact`. In phase 1 they were, because their text
folded completely, but the value a run reads is not the default.

| Fixture | Phase 1 | Phase 2 | Expressions lowered / gaps |
|---|---|---|---|
| B | 116 / 125 (93%) | 123 / 125 (98%) | 482 / 4 |
| C | 79 / 91 (87%) | 87 / 98 (89%) | 167 / 4 |
| D | 26 / 42 (62%) | 29 / 42 (69%) | 40 / 2 |
| E | 199 / 274 (73%) | 288 / 288 (100%) | 1353 / 0 |

The denominators grew. A component on a row link is now part of the flow even when its category is not a data
category (`tJavaRow`, `tJavaFlex`, `tFlowToIterate`, …), so it is lowered or counted as a gap, never left out.

Every expression lowered on B–E now prints SQL that parses. A fixture test checks it.

---

## What was built

| Part | What |
|---|---|
| `talend-rel/src/sql.rs` (new) | SQL → `etl-expr` by whitelist, in the engine's dialect. It includes Snowflake's `IFF`, `IFNULL`, `HASH`, `TO_VARIANT` and `TO_DATE` (formats of numeric fields only), and `DECODE`. Text compared with a number is written as text, and `/` on integers keeps the fraction where the source engine does. It also parses `GROUP BY` and `ORDER BY` clauses and finds aggregates. |
| `lower.rs` `elt_map` | The inputs are renamed `<alias>__<col>`. Joins work in three ways: a plain equality is a key; a key against a literal filters the joined input; a key against an expression is computed on the left first. An inequality is a condition, inner joins only. Then come `WHERE`, then `GROUP BY` (keys and aggregate arguments projected, the `Aggregate`, then the outputs over keys and `__agg_<n>`, matching keys as subexpressions), `ORDER BY`, and the projection. |
| `lower.rs` `statement` | `tDBRow` and `tXRow` become `Op::Statement`. `UPDATE` and `DELETE` have their `SET` and `WHERE` lowered; `INSERT`, `MERGE` and `TRUNCATE` keep their verb and tables. |
| `lower.rs` `java_row` | `output_row.c = …` becomes a `Project`, and a column it does not assign is null. `context.v = input_row.c` becomes a `ContextFromData` record. `System.out` printing is dropped. Anything else is a gap. |
| run-time context | A query or statement that reads a variable set from a row is `rewritten`, with a note naming the `tJavaRow` and the column (the watermark). One replaceable through an implicit context load or a `tContextLoad` is `rewritten` too. |
| `Numeric.sequence` | A `Window` sequence column. A tMap variable or filter reads it over every row; an output column, after its filter. It is ordered by a `tSortRow` straight upstream when there is one, and stays `rewritten` either way because of ties. A `tRowGenerator`'s rows are numbered before its columns are computed. That is exact: the rows are alike before they are numbered. A name called more than once in a job is a gap. |
| `routine.rs` (new) | It recognises a custom routine whose body is a digest (MD5, SHA-1 or SHA-256), as two zero-padded hex digits per byte. It reads the case (`%02X` is upper), whether the body checks for null, and the charset. |
| `tSampleRow` | A line number, then a filter on the listed numbers and ranges. |
| `etl-rel` `Op::Statement` | The new shared node. `render` refuses it as a relation: RFC 0268 places it as a hook. |

## Implementation details worth remembering

- **Snowflake names are case-insensitive.** The ELT column lookup falls back to a match that ignores case, when
  only one column matches.
- **sqlparser in a debug build overflows the stack** on a long `||` chain, such as a `HASH` over twenty
  `TO_VARIANT`s. The fixture check parses on a thread with a 256 MB stack.
- **`lower_job` now takes the project's routines:** `lower_job(job, &project.routines, catalog)`.

## Decisions

- **`HASH` becomes the MD5 of the same values as text, `rewritten`.** No other engine computes Snowflake's hash.
  The keys are new values, but the same rows get the same key. The differential run must leave these columns out.
- **`UNIQUE_MATCH` stays `rewritten`.** Fixture B's tMap code hands the mode to `AdvancedMemoryLookup`, a library
  class whose source the fixture lacks. The one lookup source it does have, tXMLMap's `DocumentLookupCache`, keeps
  the last row put. A gated test pins both.

## What remains a gap, by design or for later

- **By design:** `TalendDate.getPartOfDate("WEEK_OF_YEAR")` (it depends on the locale) and
  `TalendString.getAsciiRandomString` (it is random).
- **Not lowered yet:**
  - the functions `TalendDate.formatDateInUTC`, `TalendDate.getLastDayOfMonth`, `String.replaceAll`, and an
    array's `.length`;
  - the components `tNormalize`, `tMysqlSCD`, `tJavaFlex`, `tJava`, `tContextLoad`,
    `tFlowToIterate` / `tIterateToFlow`, `tExtractJSONFields`, `tRESTClient`, `tDataprepRun`,
    `tDataStewardshipTaskOutput` and `tMSSqlCDC`.

---

## Knowledge Captured

- **Watermarks.** A Talend watermark is a `tJavaRow` that copies a query's column into a context variable:
  `context.last_loaded_date = input_row.LAST_LOADED_DATE`. A later subjob's query is then built from it. Folding
  that query with the context's default looks complete, and is wrong.
- **Hex case.** `String.format("%02X", b)` writes upper-case hex. A routine named `generateMD5` is not
  `lower(md5(x))`.
- **Join conditions.** `render` applies a `Join`'s `condition` as a `WHERE` after the join, so a non-key
  condition is only correct for an inner join.

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/etl-rel/src/{model,render}.rs` | `Op::Statement` |
| `crates/talend-rel/src/sql.rs` | new: SQL expressions |
| `crates/talend-rel/src/routine.rs` | new: digest routines |
| `crates/talend-rel/src/expr.rs` | `Scope.routines` / `Scope.sequence`, `lower_parsed`, `sequences` |
| `crates/talend-rel/src/lower.rs` | ELT maps, statements, `tJavaRow`, run-time context, sequences, `tSampleRow`, row-link membership |
| `crates/talend-rel/tests/phase2.rs` | new: eight synthetic-job tests |
| `crates/talend-rel/tests/fixtures.rs` | E's phase 2 exit, B's `UNIQUE_MATCH` evidence, printed SQL parses |
| `Cargo.toml`, `Cargo.lock` | `ekos-talend` pinned at EKOS `859708c4` |

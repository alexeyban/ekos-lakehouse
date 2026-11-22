# Devlog 28 — Talend patterns to dbt: snapshots, keyed replace, rejects, write-back (EKOS RFC 0268 phase 2)

**Date:** 2026-11-22
**PRs:** none (direct to `main`); EKOS devlog_352
**Branch:** `main`

---

## Summary

Phase 2's pattern idioms. A Talend job keeps state in its targets, so in phase 1 many gold models were disabled:
two writers of one table, or a job that reads what it writes. Now `talend-dbt` recognises the shapes behind them,
by structure, in one job's IR (`patterns.rs`). Each recognised shape names its rule. A shape that only looks like
one is refused, and the refusal names the difference.

Measured first on the fixtures, then built:
- **SCD2 by hand** becomes a dbt snapshot.
- **Delete the matched rows, then insert the staged ones** becomes one merge on the key.
- **One rejects table, many writers** becomes one `rej_` model.
- **A flag written back on the job's own source** has no model.
- **A lookup on the target** reads `{{ this }}`.

The run also found five defects that no gate saw. One is in `etl-dbt` and four in `talend-rel`; all are fixed, and
a new scope gate catches the first kind.

| Fixture | Phase 1 enabled | Phase 2 enabled |
|---|---|---|
| B | 10 / 73 | 18 / 44 (2 / 5 snapshots) |
| C | 42 / 61 | 42 / 61 |
| D | 9 / 23 | 9 / 23 |
| E | 17 / 184 | 25 / 164 |

(Plan synthetic as in phase 1; the fixtures run with `--locale fr_FR`.)

---

## What was built

| Part | What |
|---|---|
| `talend-dbt::patterns` | `recognise(&mut Ir)`: snapshot, keyed replace and write-back groups. Each group's sinks are replaced by a virtual projection and sink appended to the IR, which the gold loop writes by the group's rule. Refusals become `pattern` questions. |
| Snapshots | `dbt/snapshots/talend/<pkg>/<model>__history.sql`: `strategy='check'`, `unique_key` = the lookup key, `check_cols` = the compared columns. The gold model is a view that gives the table's columns back: the flag from `dbt_valid_to IS NULL`, the clock columns from `dbt_valid_from` / `dbt_valid_to`, a database-assigned key from `dbt_scd_id`. A disabled snapshot is comments only. |
| `rej_` models | Writers of one table with the same keys and columns, upserts (or inserts), none reading the table, each from a reject branch or a lookup. Each part is rendered in its own job and gated there, then `UNION ALL`, merged on the keys. |
| Self-read | A lookup on the table the only writer loads. The source is inlined into each model that reads it: `{% if is_incremental() %}{{ this }}{% else %}(typed empty relation){% endif %}` in the model that writes the table, its `ref()` in the others. |
| Scope gate | Every column a projection, filter, lookup or join reads is a column of its own input. |
| `Report.patterns` | Recognised and refused, with the model, the components and the notes. New rules: `snapshot`, `keyed-replace`, `rejects`, `write-back`; new layer: `snapshot`. |
| `talend-rel` | `Options { locale }` and `lower_job_with`. `Locale` lists the known locales. Also: a `tJava`-set `NB_ROWS`, `getLastDayOfMonth`, `getFirstDayOfMonth`, `compareDate`, and month and day names under a stated locale. |
| `lakehouse talend translate` | `--faithful <job>` (repeatable, `all`), checked against the project's jobs; `--locale <tag>`, checked against `Locale::KNOWN`. The translation runs on a 64 MiB stack. |

## Implementation details worth remembering

- **Recognition reads the IR's structure, not names.**
  - A snapshot needs three sinks on one table:
    - an insert from `Filter(lookup key IS NULL)` over a left lookup on a `Source` of the table;
    - an update through a comparison filter;
    - an insert through the same comparison.
  - Both inserts must be projected identically, and the flag must be a different constant in the inserts and in
    the update.
  - The update's key is either the lookup key or a key no insert writes, a `SERIAL`, which becomes `dbt_scd_id`.
- **Write-back requires the table to be read on the job's main flow** (`main_reads`). The SCD2's closing update also
  sets only literals, but it reads the table through the lookup. The first version of the rule took it for a
  write-back.
- **The jOdsClient refusal is counted without `$path`.** With `$path` carried by tMap outputs, an "all NULL but one"
  row has one more column.
- **A `rej_` writer can be a lookup's matched rows** (jOdsVente rejects sales already loaded), so a lookup in the
  flow counts as a reject branch.
- **The empty relation of a self-read is one line**, because the Trino check keeps the first-build branch of an
  inline `{% if %}`.

## Decisions

- **Refuse, don't guess.** jOdsClient's "close" inserts a NULL row, and jOdsSousCategorie inserts a new version of
  every matched row. Both differ from an SCD2, so both stay phase 1 with a question that states the difference.
  Everything downstream of them stays disabled, which is most of B's DWH. That is the honest state until a reviewer
  answers.
- **The snapshot keeps history that Talend overwrites.** The ODS jobs' closing update matches on the business key
  and writes NULL into the attributes of every version. The snapshot does not reproduce that; the `snapshot`
  question says so.
- **`--locale` instead of an ISO-weeks flag.** The calendar needs both the week rule and the month names. Both
  follow the JVM's locale, which the project does not state, so the operator states it, from a closed list.
- **`--faithful` is phase 1's shape:** one model per output, no recognition.

---

## Knowledge Captured

- **The gates checked names, not scope.** Anti-invention accepts any column some node of the job has. A tMap that
  read `$path` after the flow had dropped it passed every gate, and would have failed in the warehouse. The scope
  gate checks each node against its own input.
- **The PostgreSQL → Trino rewrite aliases each table by its name, so `schema.table.column` must become
  `table.column`.** Trino resolves a qualifier against the alias only. Fixture B's every query used the three-part
  form. The Pentaho manifests were unchanged, so no Kettle sample does.
- **A Talend component on an existing connection keeps `""` in its own `SCHEMA_DB`.** Take the connection's value
  when the component's folds to empty, not only when the parameter is absent.
- **A test thread's 2 MiB stack is too small for the SQL parser on fixture B,** whose `rej_` model unions 18 long
  concatenations. So is a tokio worker's. Tests run the fixtures on an 8 MiB thread (a main thread's), and the
  command on 64 MiB.
- **dbt accepts a snapshot file holding only comments**: no block, no node. That is how a disabled snapshot is
  written; a `config()` outside a snapshot block would not parse.

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/talend-dbt/src/patterns.rs` | new: recognition, and its unit tests |
| `crates/talend-dbt/src/lib.rs` | groups, snapshots, `rej_` unions, self-read, scope gate, settings |
| `crates/talend-dbt/tests/{fixtures,translate}.rs` | phase 2 exit on B and E, `--faithful`; 8 MiB thread |
| `crates/talend-rel/src/{lower,expr,lib}.rs` | options and locales, `tJava` row count, date routines, `$path` through tMaps, connection fallback, typed date context |
| `crates/talend-rel/tests/phase2.rs` | calendar, locale, `$path`, connection tests |
| `crates/etl-dbt/src/pgsql.rs` | `schema.table.column` → `table.column` |
| `crates/lakehouse-ext/src/{lib,talend_cmd}.rs` | `--faithful`, `--locale`, large stack |
| `README.md` | the flags |

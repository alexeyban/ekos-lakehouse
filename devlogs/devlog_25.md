# Devlog 25 — `talend-rel`: Talend jobs as the shared relational IR (EKOS RFC 0267, phase 1)

**Date:** 2026-11-21
**PRs:** none (direct to `main`); EKOS devlog_349
**Branch:** `main`

---

## Summary

The first translation step for Talend: an expanded `ekos_talend::Job` becomes an `etl_rel::Ir`, the same IR the
Pentaho translator prints dbt from (RFC 0266). The new crate `talend-rel` has two parts:
- `expr` lowers Talend's Java expressions over `etl-expr`'s parser, with Talend's rules (plain Java, unlike
  Kettle's);
- `lower` turns components into nodes.

Every place the SQL differs from the Java is named in the node's notes, and everything else is a gap with its
text. On the five real projects:

| Fixture | Data components lowered | Expressions lowered / gaps |
|---|---|---|
| B (Open Studio, PostgreSQL) | 116 / 125 (93%) | 475 / 11 |
| C (joblets, Snowflake) | 79 / 91 (87%) | 144 / 6 |
| D | 26 / 42 (62%) | 40 / 2 |
| E (ELT, five engines) | 199 / 274 (73%) | 337 / 12 |

On fixture B, `FAIT_VENTE.NB_MT_VENTE` is written from a `sum` whose subject a map computes as
`QTE_VENTE * PRIX_VENTE`, and every tMap of the DWH jobs lowers.

---

## What was built

| Piece | What |
|---|---|
| `expr.rs` | `Scope` (column, `Var`, `context`, `jobName`, `globalMap` lookups), `talend_type`, `lower` |
| `expr.rs` rules | `equals` → `IS NOT DISTINCT FROM`; `==` on strings flagged as reference equality; `+` on a string → `concat(coalesce(x,'null'))`; `getPartOfDate("MONTH")` is zero-based; `getCurrentDate` → the run's start; `matches` anchored; `intValue` truncates |
| `expr.rs` catalogue | the routine catalogue (`TalendDate`, `StringHandling`, `Relational`, `Math`, …) and the closed `java.lang` list |
| `lower.rs` sources | database input (folded SQL, rewritten when it has run-time holes), ELT input, file input (as a landed table, rewritten) |
| `lower.rs` the tMap | every input renamed `<input>__<col>`; `Lookup` per lookup (UNIQUE_MATCH: the last match, FIRST_MATCH: the first; rewritten until a key is proven unique) or `Join` (ALL_MATCHES); per output a `Filter` and a `Project`; reject and inner-join reject outputs |
| `lower.rs` other components | tFilterRow (+ REJECT), tUniqRow (+ DUPLICATE through a per-key row number), tAggregateRow, tSortRow, tFilterColumns, tUnite, tJoin, tRowGenerator, tFixedFlowInput, the tHash hand-over, `Sink` by data action |
| `tests/fixtures.rs` | coverage on fixtures B–E, and fixture B's exit |
| `tests/pure.rs` | no file, network or process access in `src/` |
| `ekos-talend` | pinned by `rev` (it depends on no other EKOS crate), so it can move ahead of the other EKOS crates |

## Implementation details worth remembering

- **`globalMap` keys.** A key every `tSetGlobalVar` sets to a constant is that constant. A `tFileList`'s
  `CURRENT_FILEPATH` / `CURRENT_FILE` is read from the landed table's `$path`, `rewritten`. Any other key is a gap
  naming it.
- **`jobName`** is the job's own name, as Talend sets it.
- **Disconnected components.** A component with no incoming row link that produces no rows is outside any flow
  and goes on the skipped list. A SQL statement component (`tDBRow`, `tSnowflakeRow`, ELT) runs from a trigger
  and changes data, so it is never skipped. Skipping those first pushed fixture E to a false 86%.
- **The pin.** EKOS `89cedb25` lifts a component-framework input's manual query into `QUERY`. Fixture E's 14
  Snowflake inputs had neither a query nor a table before it.

## Remaining gaps (phase 2 and later)

| Gap | Count |
|---|---|
| ELT maps (SQL expressions through sqlparser) | 21 in E |
| `tSnowflakeRow` / `tDBRow` statements | 42 in E |
| `Numeric.sequence` (row order) | 7 in B |
| custom routines (`HashUtils.generateMD5`, `HashUtil.*`) | 13 in C and E |
| `tNormalize`, `tSampleRow`, `tMysqlSCD` | 1 each in D |
| locale-dependent date parts | 1 in B |
| `TalendString.getAsciiRandomString` | 3 in B and D |

---

## Knowledge Captured

- **Cargo can take one git repository at two revisions.** A crate pinned by `rev` resolves as a separate source
  from the `branch = "main"` entries of the same repository. A self-contained crate (`ekos-talend`) can therefore
  move ahead without updating every other EKOS crate.
- **Read a full commit hash with `git rev-parse <short>`.** A hand-completed hash fails the fetch with a
  misleading network error.
- **tFilterRow's function column** holds either nothing or a Java template over `$source`, `$target` and
  `$operator`. Its "advanced" condition is often only Studio's sample comment, which is not a condition.

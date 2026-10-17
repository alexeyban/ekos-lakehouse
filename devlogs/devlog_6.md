# Devlog 6 — M4 exit met: gold marts from written descriptions

**Date:** 2026-10-17
**PRs:** none (local)
**Branch:** main
**Design:** EKOS RFC 0195 (`ekos/docs/rfcs/0195-lakehouse-gold.md`)

---

## Summary

A stakeholder writes `marts/<name>.md` — a YAML header (grain, dimensions, measures, filters)
and prose. `lakehouse gold draft` resolves every term against the deployed silver plan or asks
back; compiles what the header states (many-to-one joins only); has local Ollama draft only the
prose terms, in a reflect-and-retry loop judged by RFC 0164's anti-invention check and by
Trino's `EXPLAIN`; and proposes the mart on a branch with grain and reconciliation tests. Three
LedgerSMB marts went through it: 0 invented atoms, `dbt build` 12/12, lint clean.

---

## The three marts (LedgerSMB)

| Mart | Path | Result |
|---|---|---|
| `ledger_by_account_month` | `region` asked back ("no silver column is named `region`"); the author removed it | compiled, 14 atoms, 292 rows |
| `receivables_by_customer_month` | `average_invoice` in prose; the model's `COUNT("ar".*)` passed the atoms but Trino refused it; after one instruction fact, drafted first time | 17 atoms, 928 rows |
| `sales_by_part_month` | `revenue` in prose ("qty multiplied by sellprice, summed") → `sum(qty * sellprice)` first time | 16 atoms, 1 944 rows |

`dbt build`: 3 models, 3 grain-uniqueness tests, 6 reconciliation tests (mart total = silver
total under the same joins and filters) — 12/12. `sqlfluff`: clean. Every mart went through a
proposal branch merged by "a reviewer"; changing a mart is the same path (a new id, a new branch).

---

## What was built

| Piece | What |
|---|---|
| EKOS `migrate-logic` | `check_candidate_with(source, sql, dialect)` (additive) |
| `crates/gold` (pure, 8 tests) | parse, resolve (column / table / pattern / prose; questions with candidates), compile (FK path child→parent only), source atoms, anti-invention, `engine_sql`, model YAML, reconciliation tests, proposal and questions Markdown |
| `lakehouse gold draft` | committed description required; questions file or proposal branch; Ollama drafts (`gold-model`, default `llama3`), up to 3 attempts with feedback; `EXPLAIN` on every draft and the final mart; `LakehouseMart` fact |

## Knowledge Captured

- **An LLM draft can invent nothing and still not run** (`COUNT("ar".*)`): check atoms *and*
  plan it on the engine; feed both kinds of rejection back to the model.
- **One general fact beats retries**: "count rows with count(*)" turned three failed drafts into
  a first-time pass. Facts about the engine are fine in the instructions; facts about the data are
  not (they would be invention).
- **`GROUP BY 1, 2` reads as invented literals** to the atom check; group by the expressions.
- **A joined table must be on the many-to-one side**: a parent→child join repeats base rows and
  inflates totals — it is a question, not a join.
- **Drafting from an uncommitted description breaks the merge** (untracked file vs the branch's
  copy); it now requires a committed one.
- LedgerSMB keeps the posting date and approval on `transactions`, reached from `ar`, `ap`,
  `invoice` and `acc_trans` by `trans_id`.
- The local model ran next to the stack on this machine (16 GB RAM, GTX 1650 4 GB): fine for
  three marts; no paid call was made.

## Not done

Confirmed RFC 0170 concepts as a resolution source (none are confirmed in the sandbox), Confluence
intake, metered providers.

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/gold/` | new crate |
| `crates/lakehouse-ext/src/{gold_cmd,facts,settings,sync,lib}.rs` | `gold draft`, `LakehouseMart`, settings |

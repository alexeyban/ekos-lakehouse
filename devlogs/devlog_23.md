# Devlog 23 — `lakehouse gold ask`: a question's terms, triaged against the plan and the marts (EKOS RFC 0248)

**Date:** 2026-11-07
**PRs:** none (direct to `main`); EKOS devlog_326/327
**Branch:** `main`

---

## Summary
First slice of the engine behind the public demo's step 8: a model turns a question into **terms**, and this
crate checks them, triages the request and compiles them with the machinery step 4 already has. No model call
and no write yet: `lakehouse gold ask <request.json>` reads the deployed plan and `marts/*.md` and prints the
triage, and the SQL with the anti-invention verdict when the terms resolve.

## What was built
| Piece | Where |
|---|---|
| `AskRequest` (terms only; unknown fields refused), `check`, `to_spec`, `triage`, `Change` | `crates/gold/src/ask.rs` |
| `lakehouse gold ask <request> [--json]`, `existing_marts`, `report` | `crates/lakehouse-ext/src/ask_cmd.rs` |
| catalogue class `lakehouse gold ask` = read | `crates/lakehouse-ext/src/lib.rs` |

## Decisions
- **A request has no field that can carry SQL.** `deny_unknown_fields` on a struct of `name`/`definition`
  terms; a definition may not hold quotes, `;`, comment markers, backticks, backslashes or control characters.
  The model's job ends at terms; `resolve` and `compile` (RFC 0195) turn them into SQL.
- **Prose terms are asked back.** A prose term is one a model would have to write SQL for; in `gold draft`
  that is allowed (and judged by the anti-invention check), in a question it is not.
- **The mart is named by its content** (`ask_<12 hex of sha256>`): the same terms, in any spacing or order, are
  the same mart, so asking twice builds nothing twice.
- **Changing an existing mart is only ever explicit** (`target_mart`). A name collision with another definition
  is not a change request — the model picks names and may collide by accident; a new mart has its own
  namespace. Without a target, the answer is an addition (a new mart, or `extend` an existing mart of the same
  shape with new measures; nothing existing changes).
- **Unresolved terms**: something near it → `needs_clarification` with the candidates; nothing near it →
  `impossible` with the reasons. Never a guess.
- `possible_now` needs the same dimensions, the same filters and every requested measure; rolling a finer mart
  up is not attempted (it is wrong for distinct counts and averages).

## Knowledge Captured
- `tests::ledger()` is shared by `pub(crate)`; a sibling module in the same crate can use it, but only if the
  fixture function itself is visible.
- `gold::check` needs the Trino dialect parser, so anything calling it pulls `ekos-plugin-sql-dialect-trino`
  (already a dependency of `gold`).

## Not done (RFC 0248)
The model call that produces a request (`lakehouse gold ask "<question>"`), gold-change proposals and their hard
gates, the AI review as a query to EKOS, the daily limit and consent, the build queue, Superset publishing and
the reconcile of chart values, the console step-8 page. Triage statuses `unsafe` belongs to RFC 0249.

## Files Changed
`crates/gold/src/{ask.rs,ask_tests.rs,lib.rs,tests.rs}`, `crates/lakehouse-ext/src/{ask_cmd.rs,lib.rs}`.

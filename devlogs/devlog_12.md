# Devlog 12 — RFC 0205 bake-off results, JavaScript null arithmetic

**Date:** 2026-10-18 (recorded 2026-10-19)
**PRs:** none (local `main`, pushed)
**Branch:** main — 084bc75 (fix), 34a6a6c (EKOS by git), this commit (results)
**Design:** EKOS RFC 0205 §Measured

---

## Summary

The draft bake-off that devlog_11 left blocked on an expired OpenCode key ran once the key was
renewed: three Zen models, 32 gap steps and 15 ground-truth steps, $0.37 in total over two runs of
a $1.00 cap. Its first run found a bug in the deterministic translator rather than in a model.
Claude Haiku 5.5 became the `translate` route, with GPT-6 Luna as fallback.

---

## Results (`crates/lakehouse-ext/examples/bench-results/2026-10-18.md`)

| model | A drafted & planned | B drafted | B equal to deterministic | $ per run |
|---|---|---|---|---|
| Claude Haiku 5.5 | 6/32 | 15/15 | 11/15 | 0.035 |
| GPT-6 Luna | 7/32 | 14/15 | 9/15 | 0.032 |
| MiniMax M3 | 4/32 | 14/15 | 7/15 | 0.124 |

Nearly every B disagreement is on the null row (JS `null + x` is `"null"` in a string, 0 in
arithmetic; a method on null throws). Most of set A is untranslatable per row (loops over row
metadata, random numbers, Kettle API calls, password encryption), and "cannot" with a reason is
the correct answer there. Haiku's A count moved 8 → 6 between runs: read it as a ranking.

## 084bc75 — JavaScript reads a null number as 0

Run 1's ground-truth comparison disagreed with *all three* models on the same rows. The models
were right: in Kettle's Rhino, a null Number field in arithmetic or a comparison is 0. The
symbolic executor kept SQL `NULL`. Fixed in `kettle-expr` (`java.rs`, which lowers JavaScript
too; existing assertions updated); run 2 is after the fix. The fix shipped without a test of the
reported case — added here: `a_null_number_is_zero_in_javascript_arithmetic_only` (JS coalesces,
Java does not), checked to fail with the fix reverted.

---

## Knowledge Captured

- **When every model disagrees with the ground truth on the same rows, suspect the ground truth.**
  The synthetic-row comparison checks the deterministic translator too.
- Raw bench output used to go to stdout only; the tables are now committed next to the bench, so
  the RFC's numbers can be traced.

---

## Files Changed

| File | Change |
|---|---|
| `crates/kettle-expr/src/java.rs` (084bc75) | null Number → 0 in JS arithmetic and comparisons |
| `crates/kettle-expr/src/script.rs` | regression test for it |
| `crates/lakehouse-ext/examples/bench-results/2026-10-18.md` | both runs' result tables |
| `devlogs/devlog_12.md` | this record |

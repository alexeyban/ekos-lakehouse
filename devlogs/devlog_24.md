# Devlog 24 — The engine-neutral ETL IR, taken out of the Kettle crates (EKOS RFC 0266)

**Date:** 2026-11-21
**PRs:** none (direct to `main`); EKOS devlog_348
**Branch:** `main`

---

## Summary

The Talend translation (EKOS RFC 0262 Slice 3) needs what the Pentaho translator already has:
- a typed expression tree with Trino rendering;
- fidelity labels and gaps;
- a Java expression parser;
- a relational IR and its SQL printer.

Decision D2 was to extract rather than copy. Two new pure crates hold the shared part:
- `ekos-lakehouse-etl-expr`;
- `ekos-lakehouse-etl-rel`.

The Kettle crates re-export what moved, so no caller changed. The move was gated on golden manifests of the
translator's output on three corpora, captured before any code moved: all are byte-identical after it.

---

## What moved

| To | From | What |
|---|---|---|
| `etl-expr` | `kettle-expr` | `expr.rs`, `types.rs`, `stmt.rs` whole. The first half of `java.rs`: lexer, `JExpr`, `Parser`, `JType`, with `JType::numeric` / `boxed` made `pub`. From `semantics.rs`: `Fidelity`, `Gap`, `Lowered` |
| `etl-rel` | `kettle-rel`, `kettle`, `pentaho-dbt` | `model.rs` (the IR), `Catalog` / `NoCatalog`, Kettle's `Connection`, and `pentaho-dbt`'s `render.rs` (the IR → SQL printer, which read only the IR) |

What stays Kettle's:
- the condition, Calculator, Formula and script parsers;
- the Java `Lowerer` with Kettle's rules (empty string is null, null-safe comparison, null-skipping
  concatenation);
- `kettle_compare` / `kettle_concat`;
- the step lowering (`build.rs`, `steps.rs`, `source.rs`);
- the medallion layout.

Each moved module's old file is now a one-line `pub use ekos_lakehouse_etl_…::<module>::*;`. `kettle` re-exports
`Connection`, so every path that compiled before still compiles.

## The golden gate

`crates/pentaho-dbt/tests/golden.rs` records, per corpus:
- every file `translate` writes (path and SHA-256);
- the report's JSON hash;
- each transformation's IR JSON hash.

It holds hashes only. The corpora, with the number of lines in each manifest:

| Corpus | Where | Lines | Checked |
|---|---|---|---|
| `bench-gold` | in the repo | 5 | every `cargo test --workspace` (the repo has no CI workflow) |
| Pentaho's samples | `pentaho-kettle` @ `67c23984`, `assemblies/samples` (127 `.ktr`, 17 `.kjb`), sparse-fetched | 286 | locally |
| AdventureWorks | 8 transformations, 2 jobs | 48 | locally |

The manifests were captured at `95c1b02`, before the move, and checked twice for reproducibility. After the move,
all three are identical.

`crates/etl-rel/tests/neutral.rs` fails if `etl-expr` or `etl-rel` ever depends on a `kettle`, `pentaho` or
`talend` crate.

---

## Knowledge Captured

- **Inherent methods across crates.** A type's inherent methods must live in its own crate.
  `kettle-expr` had added `Expr::iso_dow` (`formula.rs`) and `Lowered::note_only_logging` (`script.rs`); both
  became free functions where they were, with the same body and the same output.
- **Splitting `java.rs`.** It splits cleanly at `NOTE_NPE`. Everything above it is syntax, everything below is
  Kettle's meaning. The only private items the `Lowerer` used from the parser half were `JType::numeric` and
  `JType::boxed`.
- **Fetching the Pentaho samples.**
  `git clone --filter=blob:none --no-checkout --depth 1 …pentaho-kettle && git sparse-checkout set --no-cone assemblies/samples`
  fetches only the samples, not the whole repository.
- **Running the golden check locally.** Set
  `EKOS_PENTAHO_GOLDEN_CORPORA="samples=<dir>:adventureworks=<dir>"`. To capture new manifests, also set
  `EKOS_PENTAHO_GOLDEN=capture`, and only before a change that must not alter output.

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/etl-expr/*` | new crate (moved code) |
| `crates/etl-rel/*` | new crate (moved code), `tests/neutral.rs` |
| `crates/kettle-expr/src/{expr,types,stmt}.rs` | re-exports |
| `crates/kettle-expr/src/{java,semantics}.rs` | the Kettle half, plus re-exports of the moved half |
| `crates/kettle-expr/src/{formula,script}.rs` | `iso_dow` / `note_only_logging` as free functions |
| `crates/kettle-rel/src/{lib,model}.rs`, `crates/kettle/src/model.rs`, `crates/pentaho-dbt/src/render.rs` | re-exports |
| `crates/pentaho-dbt/tests/golden.rs`, `tests/golden/*.txt` | the gate (commit `95c1b02`) |
| `Cargo.toml`, crate `Cargo.toml`s | new members and dependencies |

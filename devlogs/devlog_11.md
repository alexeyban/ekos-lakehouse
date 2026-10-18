# Devlog 11 — Pentaho packages into silver and gold dbt models

**Date:** 2026-10-18
**PRs:** none (local `main`, not pushed)
**Branch:** main — fb8e8ee, c793773, 33b0c2d, bcd55a6, 9061e66, 792d56c, f15896a (+ docs commit)
**Design:** EKOS RFC 0200 (umbrella), 0201, 0202, 0203, 0204, 0205, 0207
**EKOS:** built against the local checkout (`[patch]` to `../EKOS` is **active**): the seams are
in EKOS commits cee0370 … c62d197, not pushed (EKOS devlog_279)

---

## Summary

EKOS Lakehouse translates Pentaho Data Integration packages into dbt-trino models: a deep Kettle
model (every file kind, variables, links), typed expressions and a relational IR with Kettle's
semantics made explicit, a medallion split into silver and gold, symbolic execution of
JavaScript and Java code steps, model drafts for what is left (checked before they count), and a
console tab to review it all. Live on the sandbox, a three-transformation package (an SCD
dimension, a fact with a dimension lookup, an Insert/Update aggregate) was translated, planned by
Trino and run: its result equals a hand-written query (EMEA, 358,330.5, 4,949 orders). Local
llama3 drafted a JavaScript `switch` step as a `CASE` that ran on the same data.

---

## Commits

### fb8e8ee — RFC 0201 deep Kettle model + `lakehouse pentaho scan`

**What:** `crates/kettle` reads `.ktr`, `.kjb`, repository exports, `shared.xml`,
`kettle.properties`, `plugin.xml` + jars into one model: steps with an owned XML tree (password
elements dropped), hops (disabled and error hops marked), connections, parameters, variables with
provenance (`${}` and `%%`, internal directory variables), links between files by filename or
repository reference with cycle detection; a 100-type step catalogue in 12 categories.
`pentaho scan` is the redacting raw-content entry point (excluded paths honoured).

**Measured:** Pentaho's own samples 189 transformations / 20 jobs / 973 steps (100 types), 15
unknown types, 3 unresolved links (one a run-time `${FILENAME}`); AdventureWorks 8/2/74, 0 unresolved.

### c793773 — RFC 0202 typed expressions + relational IR

| Part | What |
|---|---|
| `kettle-expr` | one `Expr` with a closed function set and Trino rendering; parsers for condition trees, the Calculator table, OpenFormula, a Java expression subset |
| semantic rules | null-as-value comparisons (`IS [NOT] DISTINCT FROM` + null branches), null-skipping concat, Sunday = 1, full-match regex, Java identity `==` (gap), `"null"` in Java concat, 0-based substring, integer division |
| `kettle-rel` | one node per step (per target for routing steps), schema + fidelity; Table input SQL parsed per connection dialect; error hops, round-robin distribution, implicit unions, gap propagation, sorted-input checks, Mapping inlining |

### 33b0c2d — RFC 0204 medallion split + dbt output

Resolution through the deployed plan (connection to the lakehouse source database → silver
model; a table the package writes → its gold model, to a fixpoint); the PG → Trino whitelist
walker; silver chains + one gold model per sink as a CTE chain; materializations (table, append,
merge, update-only merge, SCD snapshot + stage + gold, Abort → test); gaps → disabled models;
`translation.json` + `proposals/pentaho-<pkg>.md`; `pentaho translate` (EXPLAIN with package refs
inlined, proposal branch, `PentahoTranslation` facts), `pentaho report`, MCP
`ekos_pentaho_translation`; `generate` no longer deletes Pentaho files.

### bcd55a6 — RFC 0203 code steps

`stmt.rs`: one statement-subset parser for Java and JavaScript; `script.rs`: symbolic execution
(variables → expressions, `if`/`else` → `CASE`, skipped rows → a predicate) with per-language
semantics; UDJC idioms (`getRow` guard, typed `get(Fields.In)`, `setValue`, `putRow` once,
parameters). Compiled plugins go through `Extensions::java_decompiler()`; tested with a fake
decompiler. AdventureWorks: 3 of 4 JavaScript steps translate.

### 9061e66 — the console's Pentaho tab (RFC 0207)

`pentaho report --package/--step` shaped for panels (graph, models, steps with ids and inputs,
questions, proposal, one step's original and SQL); `console.rs` declares the page; verified in a
dev console with a browser (`docs/console-pentaho.png`).

### 792d56c, f15896a — RFC 0205 model drafts

Requests for gap code steps with known inputs and declared outputs; strict-JSON prompt; checks
(parses, only input fields, listed functions, literals from the step's code) with findings fed
back up to 3 times; drafts become `llm-drafted` Project/Filter nodes. Ollama routes run freely;
others need `--allow-metered`.

---

## Knowledge Captured

- **Kettle passwords were leaking into core EKOS artifacts.** `Encrypted <hex>` is reversible and
  the generic redaction pattern needs `:`/`=`; fixed in EKOS (devlog_279), but anything a pre-fix
  core build observed from `.ktr` files still has them — re-observe (pipeline logic version 4).
- **`distribute=Y` with several outputs is round-robin**, not copy: each output sees a share of
  the rows. Steps with typed targets (Filter rows, Switch/case, executors) never distribute.
- **Kettle condition lists have no precedence** (`a OR b AND c` = `(a OR b) AND c`) and Group by /
  Unique rows act on *consecutive* rows: on unsorted input SQL's answer differs — labelled rewritten.
- **Mapping output connectors are inverted** relative to input ones: in `<output>`, `<parent>` is
  the child's field and `<child>` the parent's.
- **EXPLAIN of a gold model fails before its silver models exist**: inline package refs as
  sub-queries (`plannable_sql`), with a snapshot stood in as its first build (scd id = natural
  key, valid from 1900, open end) — a NULL stand-in silently empties every dimension join.
- **Rhino hands a BigNumber to JavaScript as a number**; in Java, `BigDecimal` has no operators.
- **A drafted Project is row-local**, so it lands in the *silver* model, not gold.
- **`main_with_args` installs a global logger**: once per test binary; later calls go through the
  command function with a hand-built `CommandContext`.
- **A `;`-chained gate command still commits after clippy fails** — chain the gates with `&&`.
- SVG `fill: color-mix(... currentColor ...)` rendered black in Chromium; use rgba fills.

---

## Files Changed

| File | Change |
|---|---|
| `crates/kettle/*` | new: deep Kettle model, catalogue, variables, links, coverage example |
| `crates/kettle-expr/*` | new: Expr, Calculator, conditions, Formula, Java/JS lexer + parser, statements, symbolic executor |
| `crates/kettle-rel/*` | new: relational IR, Table input SQL, step lowerings, drafts, coverage example |
| `crates/pentaho-dbt/*` | new: resolver, PG → Trino walker, rendering, translation, proposal, drafts, plannable SQL |
| `crates/lakehouse-ext/src/pentaho_cmd.rs` | scan / translate / report, decompiler seam, draft loop, graph |
| `crates/lakehouse-ext/src/{lib,facts,console,settings,plan_cmd}.rs` | commands, classes, MCP tool, facts, Pentaho page, settings, `generate` keeps Pentaho files |
| `crates/lakehouse-ext/tests/pentaho_cli.rs` | branch + `generate`, decompiler seam, drafts |
| `docs/pentaho.md`, `docs/console-pentaho.png`, `README.md` | user documentation |
| `Cargo.toml` | four crates, `roxmltree`, `[patch]` to `../EKOS` active |

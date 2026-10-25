# Devlog 13 — Workflow steps and the sign-off gate (EKOS RFC 0213 Phase 2)

**Date:** 2026-10-25
**PRs:** none (`main`, pushed)
**Branch:** main
**Design:** EKOS RFC 0213 §3.5, §3.7

---

## Summary

EKOS gained a seven-step operator workflow bar (`ekos workflow status`) and a gate for steps 4–7. This extension now
reports its steps through `EkosExtension::workflow_steps()` and, with `require-signoff = true`, refuses `generate`,
`sync` and `gold draft` until every review subject is validly signed off.

---

## What was built

| Component | What it does |
|---|---|
| `lakehouse-ext/src/workflow.rs` | `counts` (pure: marts proposed / with questions, synced tables, latest run per table and layer group), `steps` (2, 4–7), `workflow_steps`, `gate` |
| `require-signoff` | new `[extensions.lakehouse]` key, default false |
| `generate`, `sync`, `gold draft` | call `workflow::gate` right after loading settings, before reading the source |

Step mapping: 2 = documentation (todo until EKOS RFC 0214), 4 = gold marts drafted (`LakehouseMart`), 5 = tables
with a `schema_state` (written by `sync`), 6 = latest bronze/silver run per table, 7 = latest gold run per table.

## Measured (iDempiere testbed, `require-signoff = true`)

`lakehouse sync` refused: "the source model is not signed off — pii: sign-off is stale (its facts changed);
conflicts: not signed off; …". `ekos-lakehouse workflow status`: steps 4–7 `blocked`, each with this extension's own
detail after the reason. The counts read every object: 12.1 s cold, 2.2 s once cached in
`<state>/lakehouse/workflow-counts.json` (keyed by the ledger's entry and object counts).

## Decisions

- **Off by default.** Existing PostgreSQL projects have never been reviewed; turning the gate on for them would stop
  every `generate`. A project that runs the operator workflow sets it.
- **The gate is before the source is read**, so a refused command touches nothing.

---

## Knowledge Captured

- `cargo clippy --all-targets` now flags `field_reassign_with_default` in `silver/examples/docs_fixture.rs`; fixed.
  Plain `cargo clippy --workspace` did not see it (examples are not built).

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/lakehouse-ext/src/{workflow,lib,settings}.rs` | steps, gate, setting |
| `crates/lakehouse-ext/Cargo.toml`, `Cargo.toml`, `Cargo.lock` | `ekos-semantic`; EKOS at 9a9ac8d |
| `crates/silver/examples/docs_fixture.rs` | clippy |
| `README.md` | `require-signoff`, `workflow status` |

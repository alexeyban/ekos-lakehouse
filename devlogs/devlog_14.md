# Devlog 14 — Oracle as a source (EKOS RFC 0216)

**Date:** 2026-10-25
**PRs:** none (`main`, pushed)
**Branch:** main
**Design:** EKOS RFC 0216; EKOS devlog_300

---

## Summary

The lakehouse read PostgreSQL only. A `Source` trait now stands between the commands and the engine: PostgreSQL's
catalog code moved behind it unchanged (the Pagila pipeline is byte-identical, 84 files), and an Oracle source reads
`DBA_*`/`ALL_*` through EKOS's guarded `ekos-ora-live` session, using the project's `[[databases]]` entry. The bronze
plan carries the engine; Oracle columns go through their own type map. On EKOS's iDempiere testbed: 901 of 903 tables
planned, 3,616 files generated, and 12 tables — documents, lines, master data, the 26,006-row `AD_COLUMN`, an empty
table — loaded through Trino and reconciled green at V1–V3 with the planted control.

---

## Source-engine seam and the Oracle source

### What was built

| Component | Change |
|---|---|
| `bronze/src/model.rs`, `plan.rs` | `SourceEngine` in `PlanSettings`, serialized only when not `postgres` |
| `bronze/src/typemap_oracle.rs` | Oracle → Iceberg type map, a test per rule |
| `bronze/src/mode.rs` | `choose_for(…, engine)`: Oracle `DATE`/`TIMESTAMP`/integer `NUMBER` can order a watermark |
| `lakehouse-ext/src/source.rs` | `trait Source` (`read_tables`, `reader`, `writes_refused`); `Postgres`; `:NEW.` assignments |
| `lakehouse-ext/src/source_oracle.rs` | the Oracle catalog (tables, columns, keys, FKs, comments, update triggers), `NUMBER` measurement in one scan per table, `EngineReader` in `Dialect::Oracle` (refuses a non-AL32UTF8 database) |
| `settings.rs`, `plan_cmd.rs`, `reconcile.rs`, `doctor.rs` | `source-engine`; an Oracle `source-database` names a `[[databases]]` entry (PostgreSQL keys refused); the source dialect and `digest_columns` from the plan; the doctor checks the entry's host |
| `Cargo.toml` features | `oracle` on `ekos-lakehouse-ext` and `ekos-lakehouse` |
| `deploy/trino/catalog/idempiere.properties`, compose `ORA_PASSWORD` | the testbed as a Trino `oracle` catalog |
| `tests/oracle_live.rs` | Oracle canonical forms/hashes/buckets = Rust; plan → load → reconcile, 12 tables |

### Decisions
- Keep Oracle's names in the plan. The direct reads need them; Trino folds identifiers, so nothing generated changes.
- An unconstrained `NUMBER` with no values lands at scale 10 instead of refusing its table: 30 tables with rows were
  refused for one all-NULL amount column. Safe only because Oracle's decimal reconciliation never rounds (EKOS).

---

## Knowledge Captured

- Trino's Oracle connector: unconstrained `NUMBER` → Trino `number` (casts to `decimal`), `DATE` → `timestamp(0)`.
  The Oracle container binds 127.0.0.1, so Trino reaches it only on a shared network:
  `docker network connect ekos-lakehouse_default ekos-oracle-xe-1`.
- `CAST(number AS decimal(38,2))` rounds `1.005` to `1.01` without an error.
- `check_read_only` prints every parsed query back; a `||` chain over 92 columns overflowed a debug test thread. EKOS
  now renders Trino's row string as flat `concat` calls.
- The sandbox had lost the `m3` database the RFC 0194 live test uses: `CREATE DATABASE m3` as `ekos` restores it.
  LedgerSMB is not in the sandbox any more, so its gate did not run.
- With a local EKOS `[patch]` in a `--config` file: `cargo clippy --config …` (after the subcommand), and `cargo fmt`
  rewrites `Cargo.lock` back to git.

---

## Files Changed

| File | Change summary |
|---|---|
| `crates/bronze/src/{model,plan,mode,lib}.rs`, `typemap_oracle.rs` | engine in the plan, Oracle type map |
| `crates/lakehouse-ext/src/{source,source_oracle,settings,plan_cmd,reconcile,doctor,lib}.rs` | the seam and the Oracle source |
| `crates/lakehouse-ext/tests/oracle_live.rs`, `crates/trino/tests/live.rs` | Oracle live suite; `digest_columns` |
| `Cargo.toml`, `crates/*/Cargo.toml`, `Cargo.lock` | `ekos-ora-live`, `oracle` features, EKOS 899da03 |
| `deploy/docker-compose.lakehouse.yml`, `deploy/trino/catalog/idempiere.properties` | Oracle catalog |
| `README.md` | Oracle sources |

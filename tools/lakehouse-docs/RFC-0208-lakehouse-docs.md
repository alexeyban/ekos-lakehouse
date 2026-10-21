# RFC 0208 — EKOS Lakehouse: generated documentation and wiki publishing

**Status:** Draft
**Created:** 2026-10-21
**Builds on:** RFC 0190 (Lakehouse), 0192 (bronze), 0193 (silver), 0194 (schema changes), 0195 (gold), 0196 (monitoring)
**Code:** `ekos-lakehouse/tools/lakehouse-docs` (Python, outside the core)

## Goal

A data team gets the documentation set a data project is expected to have — source and target
models with ER diagrams, lineage, business and architecture descriptions, quality and
reconciliation results — generated from what the lakehouse already knows, kept in step on every
run, and published where the organisation reads documentation: Confluence, an Azure DevOps wiki
or a GitHub wiki.

## Principle kept

The generator states only what a fact says. Inputs are the deployed plan, the generated dbt
project, mart descriptions, proposals, git history and the JSON the `lakehouse` commands already
print. It never queries a database. What only a person knows becomes a manual section that
regeneration preserves; it is never filled with an invented sentence.

## Design

1. **Inputs → model.** `lakehouse.lock.json` gives source tables, columns, keys, FKs, comments,
   PII candidates, type mappings and load decisions. `dbt/models/**` (+ `target/manifest.json`,
   `catalog.json` when present) gives silver/gold models, tests and their `meta.ekos_rule` /
   `evidence`. Evidence: `status`, `tables`, `proposals`, `check`, `reconcile` (`--json`) and dbt
   `run_results.json`, saved by `lakehouse_docs collect` into `docs/_evidence/`.
2. **Column lineage.** source → bronze from the plan mapping; bronze → silver → gold from the SQL,
   parsed with sqlglot (Trino dialect) after resolving `source()`/`ref()`/`var()`, `select *`
   expanded with upstream columns. Name-matched fallbacks are marked; unresolved columns are
   listed and fail `--strict`.
3. **Pages.** Markdown + Mermaid, front matter `title`/`order`, folder = hierarchy. Sections:
   home, business + glossary, architecture + ADRs, source (+ ER, per table), target (bronze;
   silver + ER + per model; gold + per mart), lineage (+ STTM), quality (scorecard,
   reconciliation, tests, code quality, health), governance (dictionary, PII), operations
   (runbook, SLAs + generated data contracts, schema changes, change log).
4. **Publishers.** Three standalone scripts sharing `wiki_common.py`: idempotent (content hash),
   `--dry-run`, `--prune` limited to pages they created, refuse to overwrite pages they did not
   create. Mermaid native on GitHub and Azure DevOps; PNG attachments (mermaid-cli) on Confluence.

## Follow-ups

* `ekos-lakehouse lakehouse docs` — a thin command that runs collect + generate, so the generated
  Dagster project can schedule it after `dbt build`.
* A read-only `lakehouse reconcile --history --json` from the ledger, so documentation can show
  the last reconciliation without running a new one.
* Confirmed LinkML concepts (RFC 0170) as glossary entries once silver docs consume them (M2b).
* Mermaid size limits: very large schemas are grouped already; a per-schema split of silver ER
  diagrams may be needed past a few hundred tables.

## Test plan

Fixture produced by the real bronze/silver generators (`crates/silver/examples/docs_fixture.rs`)
from a Pagila subset; 17 tests: every page written, all internal links resolve, every Mermaid
diagram renders with `mmdc`, column lineage gold → source, masking detected from SQL, manual
sections preserved, reruns are no-ops, stale pages removed; publishers against fakes of the
Confluence and Azure DevOps REST APIs and a bare git repo (create → no-op → prune; refusal to
overwrite foreign pages).

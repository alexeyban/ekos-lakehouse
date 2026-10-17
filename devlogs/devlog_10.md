# Devlog 10 — one Dagster for every project, its own version, release 0.1.1

**Date:** 2026-10-17
**PRs:** none (pushed to `main`)
**Branch:** main (EKOS by git, `main` at 0b37141)
**Design:** RFC 0199 follow-ups

---

## Summary

Three gaps left by 0.1.0 are closed. One Dagster now serves every project in the EKOS home — each
a code location of its own, every asset key under the project's name — instead of one project
per stack. `ekos-lakehouse --version` names itself (`ekos-lakehouse 0.1.1 (EKOS 1.1.0)`). A project
added while the console runs appears without a restart (EKOS devlog_278). Verified live: `shop`
and `sales` load side by side, both schedules running, and a `sales` run in the shared Dagster
finished healthy.

---

## What was built

| Piece | What |
|---|---|
| `dagster.py` | a project with `../.lakehouse/project.json` takes its name and directory from there and prefixes every asset key (`sync`, `health`, bronze, and dbt models and sources through a `DagsterDbtTranslator`) |
| `docker-compose.home.yml` | `dagster dev -w $EKOS_HOME/lakehouse/dagster/workspace.yaml`, a shared `DAGSTER_HOME`, 3 GB for one code server per project |
| `install.rs` | writes `project.json` and `workspace.yaml`; refreshes the other projects (`generate --from-lock`, no source connection); a project that fails is reported and left out; Dagster restarted with all of them |
| `lakehouse generate --from-lock` | re-render a pipeline from its deployed plan, for upgrades |
| `binary()` | the extension's name and version for `--version` |

## Decisions
- The prefix applies only to installed projects (`project.json` present), so a pipeline run on
  its own — the quickstart's — keeps the asset keys it always had.
- Other projects are refreshed from their lock, never re-planned: an install must not need every
  other database to be reachable, nor change another project's plan.

---

## Knowledge Captured
- Dagster's workspace file cannot give code locations their own environment, so a project's
  identity has to travel with its code (`project.json` beside the pipeline).
- Duplicate asset keys across code locations are the failure to design against; dbt sources
  carry the bronze key in their `meta`, so prefixing in the translator keeps models joined to
  their bronze tables.
- Old pipelines generated before an upgrade break when loaded into a shared Dagster (they read
  `EKOS_LH_PROJECT`, now the projects directory): the installer regenerates them first.

---

## Files Changed
| File | Change summary |
|---|---|
| `crates/bronze/src/dagster.py` | project.json, key prefix, dbt translator |
| `deploy/docker-compose.home.yml` | workspace mode |
| `crates/lakehouse-ext/src/{install,lib}.rs` | workspace.yaml, refresh, `--from-lock`, `binary()` |
| `docs/{install,troubleshooting}.md` | several projects; refresh warnings |
| `Cargo.toml`, `Cargo.lock` | 0.1.1, EKOS 0b37141 |

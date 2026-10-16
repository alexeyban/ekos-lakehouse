# Devlog 1 — The repository exists; `lakehouse doctor`

**Date:** 2026-10-16
**PRs:** none (local)
**Branch:** main

---

## Summary

EKOS Lakehouse starts as a separate repository holding an EKOS extension (RFC 0190). EKOS
gained what the extension needed the same day (EKOS devlog_274: extension seam v2 — commands,
catalogue classes, scoped MCP tools, `[extensions.<name>]` settings — plus a Trino dialect and
Trino canonical-hash expressions). This repository has the binary `ekos-lakehouse` and one
command, `lakehouse doctor`, which proves the seam end to end.

---

## What was built

| Component | What |
|---|---|
| `Cargo.toml` | workspace; EKOS by git (`main`), patched to `../EKOS` for every EKOS crate |
| `crates/lakehouse-ext` | `LakehouseExtension`: the `lakehouse` command group; settings from `[extensions.lakehouse]` |
| `crates/lakehouse-ext/src/doctor.rs` | Trino `/v1/info` (not starting), Polaris health `UP`, MinIO `/minio/health/live`, Dagster `/server_info`, source TCP connect; fails when any configured service fails or nothing is configured |
| `crates/ekos-lakehouse` | the binary |

Checked by hand: `--help` lists `lakehouse`; `commands --json` classifies `lakehouse doctor` as
`external`; with Trino and the source unreachable, doctor reports both and exits 1.

---

## Knowledge Captured

- **Seed `Cargo.lock` from EKOS's.** A fresh resolve fails: every `zip 2.x` release is yanked,
  and only EKOS's existing lockfile still pins one. Copying `../EKOS/ekos/Cargo.lock` also keeps
  this workspace on exactly the versions core is tested with.
- **Patch every EKOS crate or none.** Patching only the crates named directly would let the rest
  resolve from git and mix two copies of the shared types.

---

## Files Changed

| File | Change summary |
|---|---|
| everything | new |

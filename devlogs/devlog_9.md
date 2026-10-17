# Devlog 9 — the console tab, a one-command install, and the docs to go with them

**Date:** 2026-10-17
**PRs:** none (local)
**Branch:** main (EKOS by git, `main` at 1f0a4dc with RFC 0198/0199)
**Design:** EKOS RFC 0198 (console pages from extensions), RFC 0199 (this repository's part)

---

## Summary

EKOS Lakehouse now has a web UI and an installer. The UI is a **Lakehouse** tab in the EKOS web
console, declared by this binary as data (EKOS RFC 0198): health per table and layer, proposals
waiting for a merge, how each table loads, refused tables, the stack — and buttons for doctor,
sync, check, plan and generate. `curl … install.sh | sh` installs the release's full package and
runs `lakehouse install`, which writes and starts the stack, creates a project (the sample
`shop`, or `--source` for your own PostgreSQL), runs the pipeline once, starts Dagster on its
schedule and sets up the console in local mode. Verified end to end from a package, in a scratch
home: sample and an external database both green in bronze and silver, Doctor run from the
console with every check passing.

---

## The console tab

### What was built
| Piece | What |
|---|---|
| `console.rs` | `pages()`: five panels, five actions; `tables` and `proposals` (pipeline repository only: lock file, git branches) |
| `lakehouse tables --json`, `lakehouse proposals --json` | new read-class commands the panels run |
| `LakehouseExtension::console_pages` | the seam hook (EKOS RFC 0198) |

### Decisions
- Panels read the pipeline repository and the ledger only, so they are class read and safe to
  refresh; anything touching the source or Trino is an action with its own class and
  confirmation.

---

## The installer

### What was built
| Piece | What |
|---|---|
| `install.sh` | POSIX sh: resolve the release, verify `SHA256SUMS` before unpacking, install `bin/` + `console/` into the EKOS home by rename, exec `lakehouse install` |
| `lakehouse install` (`install.rs`) | stack files compiled into the binary; Docker/port checks; sample DB or `--source`; home project; plan, generate; Dagster image, first run, schedule; console via `setup --answers`; `--uninstall [--purge]` |
| `docker-compose.home.yml` | override for a home project: EKOS home mounted at the same path, `EKOS_LH_PROJECT_NAME` |
| `source-password-file` | the setting the installer writes; the password itself in `.lakehouse/source-password` (600) |
| `scripts/release.sh`, `docs/INSTALL-package.md` | the full package via EKOS's `package-full.sh` under the binary's own name |
| Trino `secrets.env` (600) + `host.docker.internal` | an external source's password and a database on this machine |

### Found while building it
- **Console jobs have no shell variables.** Doctor from the console failed on the source
  password (`EKOS_LH_PG_PASSWORD … not set`): the console gives a job only its tenant's granted
  variables, and grants are tied to metered model calls. A password file (EKOS
  `ConnectionRef::secret_file`, refused if others can read it) works in console jobs and in the
  orchestrator's container alike.
- **Dagster runs a project in the home only with `--project`.** The generated `definitions.py`
  prefixes every call with `--project $EKOS_LH_PROJECT_NAME`; the home is mounted at the same
  path by an override file (mounting it in the base file would create `/home-not-set` as root,
  or mount the project twice, when the variable is unset).
- **`docker compose up --wait` fails on one-shot containers** (bucket, catalog setup); readiness
  is polled on Trino's `"starting":false` and Polaris' health instead.
- **`DAGSTER_HOME` must exist** before `dagster job execute`, or Docker creates it as root.
- **Dagster on the host network has no Docker port**: the port check takes the stack's own
  `lh-dagster` by name; and `docker inspect`'s first variable shared the `true` line — the
  one-Dagster-one-project guard missed it until the state got its own line.
- **"Text file busy"**: the binary is mounted into Dagster and run by the console; it can only be
  replaced by rename (as `install.sh` does).
- The sample's `orders` needs its watermark declared to load incrementally; new sample installs
  declare it.

---

## Knowledge Captured
- A console job's environment is not your shell's: put what a job needs in an owner-only file the
  project names, or grant it to the tenant.
- One Dagster, one project: the installer never takes a schedule away from another project.
- Test an installer with a scratch `HOME` and `DOCKER_CONFIG` pointing at the real one (the
  Compose plugin lives there), and a `file://` release directory (`EKOS_LH_BASE_URL`).

---

## Files Changed
| File | Change summary |
|---|---|
| `crates/lakehouse-ext/src/{console,install}.rs`, `lib.rs` | the tab, `tables`/`proposals`/`install` |
| `crates/lakehouse-ext/src/{settings,plan_cmd}.rs` | `source-password-file` |
| `crates/bronze/src/dagster.py` | `--project` for home projects |
| `deploy/docker-compose.lakehouse.yml`, `deploy/docker-compose.home.yml` | `secrets.env`, `host.docker.internal`, the home override |
| `install.sh`, `scripts/release.sh`, `docs/INSTALL-package.md` | installer and release |
| `README.md`, `docs/{install,console,troubleshooting,quickstart}.md`, `docs/console.png` | documentation |

# Devlog 30 — The Talend differential run: sandbox runner and `lakehouse talend verify` (EKOS RFC 0268 phase 3)

**Date:** 2026-11-22
**PRs:** none (direct to `main`); EKOS devlog_354
**Branch:** `main`

---

## Summary

The first verification by execution of a Talend translation. The two halves:
- **The Talend side.** Fixture B's own built jobs ran in an isolated Windows VM, the operator's libvirt sandbox.
- **The dbt side.** The translated models' first build ran on a throwaway Trino, over the same input files.

Results:

| Table | Rows | Result |
|---|---|---|
| `DIM_CALENDRIER` | 2,192 | equal, all 17 columns |
| `ODS_CATEGORIE` (snapshot) | 3 | equal |
| `ODS_TYPE_CLIENT` (snapshot) | 3 | equal |

- **Differences:** none.
- **Not verified:** 12 output tables, all downstream of the two ODS jobs refused for their defects.
- **Control:** the comparer found its planted difference.

Running it found two translation defects. Both are fixed.

---

## What was built

| Part | What |
|---|---|
| `scripts/talend-sandbox/run.sh` | Host side. It builds a payload (job jars, `temp/lib`, routines, the plan's files; a job compiled but not packaged is zipped from `target/classes`; the plan's Studio libraries from `EKOS_TALEND_STUDIO`). It reverts the VM to its clean snapshot, runs, brings back the dumps, logs, manifest and a host copy of the inputs, and reverts again, on failure too. |
| `vm-run.ps1`, `pg.ps1` | VM side. They set up a throwaway PostgreSQL on `127.0.0.1:5432`, the project's create and insert scripts, the sandbox's context rows and inputs, and run each job (`java -cp <jar>;lib\*`, with the plan's `jvm_locale`). Every table is dumped as CSV after each job. Output never goes through a pipe. |
| `fixture-b.json` | Fixture B's plan, where every value the project does not state carries its reason: job order, scripts, context rows, input paths, JVM locale, Studio libraries. |
| `talend-dbt::verify` | Pure parts: file specs from each file component (separator, header, encoding, empty rows, date patterns, Excel); a delimited and an `.xlsx` reader; PostgreSQL CSV; normalisation; multiset comparison; which columns to leave out, with why. |
| `lakehouse talend verify` | Command (`external`). It translates (empty plan), lands each `talend_files` source in Trino's `memory` catalog, builds the enabled models' first versions in ref order (a snapshot's first version emulated), compares each gold model with the table the jobs left, runs the control, drops everything, and writes `<state>/lakehouse/talend/<pkg>.verification.json`. |
| `ModelReport.clock` / `surrogate` | The target columns set from the clock, and a database-assigned key, so the comparison leaves them out by name. |
| `talend-rel` | Listed inputs named by their `tFileList` masks; a file's name cut at either separator. |

## Implementation details worth remembering

- **Both translation defects were invisible to every gate:**
  - all six of B's ODS file inputs read `globalMap.get("tFileList_1_CURRENT_FILEPATH")`, so they shared one source
    name;
  - on Windows, `$path` is `C:\…\file.csv`, and the file name was cut at `/` only.
- **One Talend run on empty tables is each model's first build:** incremental models build in full, and a
  snapshot's first version holds every row as current. That is what is compared. A second, incremental run is not
  yet.
- **`$path` is the VM's absolute path** (`to` with backslashes, then the file name), so `LB_CHEMIN_FICHIER`
  compares.
- **Trino's `memory` catalog takes `CREATE TABLE … AS`**, and the stock `trinodb/trino:483` image has it. The
  verification's schemas are prefixed with a random id and dropped afterwards.

## Decisions

- **The JVM's locale is stated in the plan (`fr_FR`), not inherited from the VM (`en_US`).** The month names
  differed until both sides used the same locale. The project does not state one, so it is a sandbox setting.
- **Libraries from the Talend Studio the project was built with**, not from the internet. The sandbox has no
  network, and those are the exact builds Talend shipped (`…_modified_talend`).
- **No compilation.** A job without a jar is zipped from its own compiled classes.

---

## Knowledge Captured

- **Windows `Start-Process` joins `-ArgumentList` with spaces and quotes nothing.** Pass SQL through a file.
- **A `Start-Process -PassThru` object has an empty `ExitCode` after exit unless `$p.Handle` was read** while it
  ran.
- **A VM keeping local time in its RTC** makes `tar` warn on every file. Extract with `--warning=no-timestamp`.
- **Talend's delimited reader takes a bare CR as a line end** (`CUSTOMER.csv`), even with `ROWSEPARATOR "\n"`.
- **Excel dates are day numbers from 1899-12-30** (`43831` is 2020-01-01).

---

## Files Changed

| File | Change summary |
|---|---|
| `scripts/talend-sandbox/*` | new: runner, VM script, cluster script, fixture B plan |
| `crates/talend-dbt/src/verify.rs` | new: the pure comparison |
| `crates/talend-dbt/src/lib.rs` | `clock`, `surrogate` on model reports |
| `crates/talend-rel/src/lower.rs` | masks name listed inputs; either separator |
| `crates/lakehouse-ext/src/{talend_cmd,lib}.rs` | `lakehouse talend verify` |
| `crates/*/Cargo.toml`, `Cargo.toml`, `Cargo.lock` | `zip`, `roxmltree`; `talend-rel`, `etl-expr` in the extension |
| `README.md` | the runner and `verify` |

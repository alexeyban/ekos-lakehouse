# EKOS x Talend "Data Integration Demos": results

Run of 2026-11-23 on the Windows VM. EKOS build: `C:\ekos\ekos-dev-x86_64-pc-windows-msvc\ekos.exe` (`ekos 1.6.1`).
Talend Studio: TOS_BD 8.0.1, project `LOCAL_PROJECT_DATA_INTEGRATION_DEMOS`.
Commands and outputs: `C:\ekos-test\out\log`. Screenshots: `C:\ekos-sandbox\screenshots`.

How this run differed from the plan:
- Commands were run by a script (`C:\ekos-test\run-ekos-test.ps1`) started by the person; Studio was driven by Claude.
- Findings and lineage were checked against the job items (the XML Studio reads), all 178 of them, plus one job opened in Studio. The plan asked for every finding to be opened in Studio.
- `didemo` was first compiled by the old installed build (2026-11-20) and then rebuilt by the new one over the same state.
- The console was checked in the EKOS desktop app, which runs the installed 2026-11-20 build, not the build from `main`.
- The MCP check from Claude Desktop (phase 5) was not run.
- Documentation and EKOS-Lakehouse were added on 2026-11-25 (section below).
- Phase 6 ran on 2026-11-24 with four jobs: the only two PostgreSQL jobs in the project, plus two file-output jobs (the other database jobs need MySQL, Oracle, SQL Server and so on). Host and port of the two PostgreSQL jobs were changed in Studio; the copy EKOS compiled was not.

## Results table

| Phase | Check | Expected | Observed | Pass |
|---|---|---|---|---|
| 1 | EKOS version and commit | 2026-11-23 or later | `ekos 1.6.1`; no commit or date printed, so the date cannot be confirmed. `talend` subcommand present | Unverified |
| 2 | Jobs / contexts / connections / routines | census | 169 jobs (170 items: `tAddCRC` 0.1 and 0.2) / 1 / 1 / 16 | Yes |
| 3 | TalendJob objects, folder source | = census | 170 rows (one per item); docs count 169 jobs | Yes |
| 3 | TalendJob objects, zip source | = folder | `project add` fails: `is not a directory` | No |
| 4 | Findings checked / false | all / 0 | 178 checked / 0 false | Yes |
| 4 | Lineage columns checked | 2 per job, 5 jobs | 6 columns on 3 jobs: 4 match, 2 have garbled origins. The 5 planned jobs write no table and have 0 lineage rows | Partial |
| 4 | Estate `talend/` pages | one per job | 169 job pages for 169 jobs | Yes |
| 5 | MCP answers matching phase 4 | 4 of 4 | Not run | n/a |
| 5 | Console Talend tab job count | = census | With `ekos-lakehouse` serving the console: Talend → dbt page lists `di-demos`, 169 jobs (screenshot 30). The plain `ekos` console has no Talend page; the tab belongs to the extension | Yes |
| 5 | Passwords found in state / control found | 0 / yes | 0 password values in the readable files / control found in 3 files. The one hit is the word used as a placeholder, matching a property name. 4 zstd files of `didemo` unread (no `zstd` tool) | Partial |
| 6 | Tables and files written vs what EKOS names | equal sets | 4 jobs, all exit 0. Tables: equal (`metadata` written, 10 rows; `tPostgreSQLInput` wrote nothing). Files: `delimited.csv` matches; `tFileOutputXML` wrote `sample0.xml`…`sample4.xml`, EKOS names `sample.xml` | Partial |

## Findings check (phase 4)

| Finding kind | Count | True | False | Reason |
|---|---|---|---|---|
| `uncalled_job` | 157 | 157 | 0 | Equals the set of jobs that no `tRunJob` calls and that call none (169 jobs, 5 called, 7 callers) |
| `absolute_path` | 19 | 19 | 0 | 11 fields hold a literal absolute path; 8 hold a context variable whose default is one |
| `repeated_message` | 1 | 1 | 0 | `tLogCatcher1` and `tLogCatcher2` both carry the same tDie message |
| `unused_item` | 1 | 1 | 0 | No job references `globalContext` by id, and it holds no variables |

The 8 `absolute_path` findings on context expressions (`beforeRunJobs.tFileUnarchive_1.DIRECTORY`, `built_inContext_1` and `built_inContext_prompt` `tFileFetch_1.DIRECTORY`, `demo02_defineProperties.tFileFetch_1.DIRECTORY`, `step1CreateTable.tFileInputDelimited_1.FILENAME`, `tContextLoad_and_file.tFileInputDelimited_1.FILENAME`, `tExternalSortRow.tExternalSortRow_1.SORT_CMDPATH`, `tFileInputMSDelimited.tFileInputMSDelimited_1.FILENAME`) are true: the variable's default is an absolute path, for example `root = C:\TalendDemoFiles\` in `beforeRunJobs` (screenshot 08). An earlier version of this file marked them false; that was a mistake in the check, corrected after opening the job in Studio.

Lineage checked:
- `scd_step2`: `employee_scd.id` from `employee.id`, `employee_scd.salary` from `employee.salary`. Match the tMap expressions.
- `tMysqlOutput`: `demotable.number`, `demotable.txt`, INSERT, no origin. Match (fed by tRowGenerator).
- `generic_elt_step1`: `customer.id`, `state.idstate`. Target and action match; origin text is garbled (defect 4).

## Phase 6 runs

Run from Studio's Run tab (the plan says Build Job and the `.bat`; the Run tab runs the same generated code). "Update Detection" offered to turn `root` into a built-in context variable before each run; it was cancelled, so the jobs ran unchanged.

| Job | Exit | Rows | EKOS: written | Written (observed) | Match |
|---|---|---|---|---|---|
| `tFileOutputDelimited` | 0 | 10 | file `context.root + "talend_files_0.1/out/delimited.csv"` | `C:\TalendDemoFiles\talend_files_0.1\out\delimited.csv`, 10 lines | Yes |
| `tFileOutputXML` | 0 | 500 | file `context.root + "talend_files_0.1/out/sample.xml"` | `sample0.xml` … `sample4.xml` in the same folder, 5 files; no `sample.xml` | No (defect 9) |
| `tPostgreSQLOutput` | 0 | 10 | table `metadata` | `myschema.metadata`, 10 rows (absent before the run) | Yes |
| `tPostgreSQLInput` | 0 | 10 read | nothing written; reads `demotable` | no table added or changed; read the 10 seeded rows of `myschema.demotable` | Yes |

Database snapshots (`C:\ekos-test\out\phase6\report-*.txt` and the pasted reports): before, `myschema.demotable` 10; after, `myschema.demotable` 10 and `myschema.metadata` 10.

Setup for the PostgreSQL jobs: PostgreSQL 16 on `127.0.0.1:55432` (`C:\ekos-sandbox\pg.ps1`), database `mysid`, schema `myschema`, role `login` as the jobs name them, and a 10-row `demotable` seeded for the input job. Host and port were changed to `127.0.0.1` / `55432` in both jobs and saved in the Studio workspace. Studio downloaded `postgresql-8.4-703.jdbc4.jar` from Maven after the PostgreSQL license was accepted (with the person's approval).

## Defects

1. **Context variables are not reported.**
   Command: `ekos --project didemo talend explain --job repositoryContext`
   Output: `repositoryContext (…): 1 components, 0 links, 0 context variables, 0 lineage rows, 1 findings`; the estate page says `Context variables: None`.
   Studio shows: two variables, `sampleInt` and `root` (from `globalContext`), default environment `demo` (screenshot 07). 80 of the 170 job items declare context parameters; every job page checked says None, including `generic_elt_step1`, whose own "files read" line uses `context.root`.

2. **`talend assess` cannot be pointed at an EKOS project with `--project`.**
   Command: `ekos --project didemo talend assess --out C:\ekos-test\out\assess.md` (and `--json`)
   Output: `Error: no Talend project `didemo` in the compiled ledger`, exit 1.
   Cause, from `ekos talend assess --help`: its own `--project <PROJECT>` means "One project (its name or label); all by default", that is the Talend project. It shadows the global `--project` that every other command uses for the EKOS project, including `talend explain`, which has a separate `--talend-project`. Way round that works: set `EKOS_PROJECT=didemo` and run `ekos talend assess --out …` (exit 0; 178 findings, 169 jobs, 0 model calls, same counts as the estate page). Naming the Talend project does not work: `ekos talend assess --project LOCAL_PROJECT_DATA_INTEGRATION_DEMOS` exits 1 with `invalid project name … use lowercase letters, digits, - and _`, because the value is validated as an EKOS project name. So the option cannot select a Talend project whose name has capitals.

3. **A Studio items-export `.zip` is not accepted as a source.**
   Command: `ekos project add didemozip --source C:\ekos-test\src\di-demos.zip`
   Output: `Error: --source C:\ekos-test\src\di-demos.zip is not a directory`, exit 1.

4. **Lineage origins from file inputs are garbled.**
   Page `jobs/generic_elt_step1.md`: origin `customer.csv")}}.Somme7` for target `customer.Somme7`.
   The item has file name `context.root+"talend_files_0.1/in/customer.csv"`; the origin looks cut at the last `/` of the expression.

5. **Withdrawn: tMap expressions are not a defect.** They were withheld by EKOS's licence gate, not lost. With a named source licensed in `[docs-estate] licences`, the pages show them (`row1.number * 10`, `StringHandling.UPCASE(row1.txt)`; screenshot 23). See defect 10 for why the gate could not be opened on `didemo`.

6. **`ekos --version` prints no commit or build date.** Output: `ekos 1.6.1`, identical for the 2026-11-20 installer and the build from `main`.

7. **EKOS desktop app stays on "Starting the console".** Its own log shows the console up on `127.0.0.1:8000` with `/api/health` 200, after `ekos console: exited (exit code: 1073807364); starting it again`. An empty terminal window opens beside it (screenshot 03). Seen with the installed 2026-11-20 build.

8. **Withdrawn: no Talend view in the console.** The Talend tab is a page of the EKOS-Lakehouse extension (`ekos-lakehouse`), which the plain `ekos` build does not include. With `ekos-lakehouse` serving the console, the Talend → dbt page lists `di-demos`, 169 jobs, 0 joblets (screenshots 30, 31).

9. **Split file outputs are named as one file.**
   Job `tFileOutputXML`: EKOS (`jobs/tfileoutputxml.md`, `explain`) says it writes `context.root + "talend_files_0.1/out/sample.xml"`.
   The component has `SPLIT = true`, `SPLIT_EVERY = 100`; the run of 500 rows wrote `sample0.xml` to `sample4.xml` and no `sample.xml` (screenshot 16, phase 6 report). Lineage and impact questions on this job name a file that never exists.

10. **A project added from a bare folder can never show its text in the docs, and cannot be fixed in place.**
    `didemo` was added with `project add didemo --source <dir>`, which records no source name. The estate docs show text only from a named, licensed source, so all 690 texts were left out ("text with no recorded source"), whatever `[docs-estate] licences` said.
    Command: `ekos --project didemo source add talend-demo --path C:\ekos-test\src\di-demos --kind dir`
    Output: refused, "this project was compiled without source names … create a new project with named sources instead".
    Way round used: a new project, `didemodocs`, added as `project add didemodocs --source talend-demo=C:\ekos-test\src\di-demos`, licensed Apache-2.0.

11. **The console from the 2026-11-19 package fails one dashboard panel under `ekos-lakehouse`.** "Objects by kind" returns `500 Internal Server Error — GET /api/workspaces/didemodocs/stats/kinds` (screenshot 29). The web app was copied from the installed package because the extension ships no Windows console; the two versions do not match.

## Documentation and EKOS-Lakehouse (2026-11-25)

All data and output on drive E: (`E:\ekos-work`). Screenshots 21–33 are in `E:\ekos-work\screenshots`.

| Item | Result |
|---|---|
| Project | `didemodocs`, home `E:\ekos-work\ekos-home`, one source `talend-demo` (same files as `didemo`), `[docs-estate] licences = { "talend-demo" = "Apache-2.0" }` |
| Estate docs | 180 pages in `E:\ekos-work\out\docs-v2\estate`, one per job; no text left out (screenshots 21–26) |
| Solution-architect and curated docs | `docs-v2\solution-architect`, `docs-v2\curated` (screenshots 27, 28) |
| `ekos-lakehouse` | built from source on the VM: 0.1.1 on EKOS 1.6.1, 60 min, `E:\ekos-work\lakehouse\bin\ekos-lakehouse.exe`. Needed Git, Rustup, CMake, NASM and the VS 2022 Build Tools; the first attempt ran C: out of space |
| `lakehouse talend scan` | `di-demos: 169 jobs, 0 joblets` — equals the census |
| `lakehouse talend report` | "No Talend translation yet" |
| `lakehouse doctor` | exit 1: no Trino, Polaris, S3, Dagster or source configured |
| Console on port 8001 (served by `ekos-lakehouse`) | Lakehouse, Documentation, Requirements, Marts, Extraction, Pentaho → dbt and Talend → dbt pages (screenshots 29–33) |
| Talend → dbt translation | not run: it needs `lakehouse generate` first (a source database with the jobs' tables, the six review subjects signed off, and Trino for the checks) |

## Observations (not counted as defects)

- EKOS does read context defaults (the `absolute_path` findings resolve through them), so defect 1 is about what `explain` and the job pages show, not about parsing.
- 157 of 178 findings are `uncalled_job`. In a project of standalone demo jobs that is true but carries little information.
- `repositoryContext` and `tContextLoad` reference a repository context id that no context in the project has. Studio offers to switch `root` to built-in mode when the job is opened (screenshot 06; cancelled, nothing changed). EKOS does not flag it.
- EKL counts 170 `TalendJob` objects, the estate pages count 169 jobs; the difference is the second version of `tAddCRC`.
- The lineage table of `scd_step2` lists some targets twice (two tMap/SCD pairs) with nothing to tell them apart.
- `commit` took 107 s for this project.
- Password scan: the single hit in `…\didemo\state\ledger\facts\manifest.json` is the plaintext placeholder from `tWebServiceInput`, a common word, found right after `"properties.has_` in a list of property names. It is a field name, not a leaked value. None of the 249 encrypted values was found. The compressed model and snapshots were not read.
- `talend explain --json` carries the finding messages in full; only the estate pages replace them with "text left out (licence)".
- Component labels come through as raw templates in `explain --json`, for example `__UNIQUE_NAME__  <br>__TABLE_ACTION__ __TABLE__`.
- `ekos --project didemo console open` fails with `'ekos-console' serves the whole home; --project does not apply`. Correct once known, but the plan's phase 5 wording suggests the flag.
- `ekos doctor` on `didemo`: all checks passed, 3,088 objects, no LLM provider configured.
- Console dashboard for `didemo` shows 3,979 objects; `ekos doctor` from the `main` build reports 3,088 for the same project (screenshot 09).
- `assess.md` written by the CLI carries the finding texts in full; the estate copy of the same assessment replaces them with "text left out (licence)".
- Console reports its version as `v0.0.0+dev`, which does not say which build is running (see defect 6).
- The "reinstall" on 2026-11-24 put back the 2026-11-19 build: `…\AppData\Local\Programs\ekos\bin\ekos.exe`, size 87,357,952, sha256 `3F9CEBC6…8AEB`, no `talend` subcommand. Both builds print `ekos 1.6.1` (defect 6).

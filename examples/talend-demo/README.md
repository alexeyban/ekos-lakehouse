# Talend "Data Integration Demos" through EKOS and EKOS-Lakehouse

Talend's own demo project (Talend Open Studio 8.0.1), compiled by EKOS on Windows, with the documentation EKOS generated from it, the test that checked that documentation against Talend Studio and real job runs, and the screenshots taken along the way (2026-11-23 to 2026-11-25).

| Folder | What it holds |
|---|---|
| `source/` | The project as exported from Talend Studio (Export items): 169 jobs in 170 job items, 1 context, 1 connection, 16 routines |
| `docs/estate/` | Estate documentation: project overview, one page per job (components, links, tables read and written, column lineage, findings), the migration assessment |
| `docs/solution-architect/` | Dependency-risk report, findings memo, onboarding guide |
| `docs/curated/` | Architecture, sequence diagrams, entity roll-ups, dependency graphs |
| `docs/assess.md`, `docs/explain-*.txt` | `ekos talend assess` and `ekos talend explain` output |
| `docs/ekos.toml` | The EKOS project configuration that produced the docs |
| `results/results.md` | The test: results table per phase, findings check, job runs against PostgreSQL, defects |
| `screenshots/` | 33 screenshots: Talend Studio, the EKOS console, the generated docs, the EKOS-Lakehouse console |
| `scripts/` | The PowerShell scripts used on the Windows VM |

## How the docs were made

EKOS shows text from a source only when the source is named and its licence is known, so the project was added with a named source and the source licensed as Apache-2.0 in `ekos.toml`:

```
ekos project add didemodocs --source talend-demo=<path to source>
# ekos.toml:  [docs-estate]  licences = { "talend-demo" = "Apache-2.0" }
ekos --project didemodocs build
ekos --project didemodocs recover
ekos --project didemodocs resolve
ekos --project didemodocs compile
ekos --project didemodocs commit
ekos --project didemodocs docs generate --layout estate --output docs/estate
ekos --project didemodocs docs generate --layout solution-architect --output docs/solution-architect
ekos --project didemodocs docs generate --layout curated --output docs/curated
EKOS_PROJECT=didemodocs ekos talend assess --out docs/assess.md
```

EKOS was built from EKOS `main` (2026-11-23), EKOS-Lakehouse 0.1.1 from this repository, built on the Windows VM. `ekos-lakehouse lakehouse talend scan` finds `di-demos: 169 jobs, 0 joblets`; the Talend → dbt translation was not run (it needs `lakehouse generate`, a source database and Trino).

## Results in short

- All 178 findings were checked against the job files: none is false.
- Four jobs were run in Studio; three wrote exactly what EKOS names, one (`tFileOutputXML`) splits its output into five files where EKOS names one.
- Open defects, with commands and output, are listed in `results/results.md`.

## Licence and changes

The Talend demo project is part of Talend Open Studio, Apache-2.0, © Talend. Two Studio user password hashes in `source/LOCAL_PROJECT_DATA_INTEGRATION_DEMOS/talend.project` were blanked before publishing; nothing else in `source/` was changed. The connection and job passwords in the items are Studio's encrypted values as shipped.

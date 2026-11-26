---
title: "tMysqlSCD"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tMysqlSCD

Folder `t01_Compo/Databases/SCD`, version 0.1. Source: `process/t01_Compo/Databases/SCD/tMysqlSCD_0.1.item`.

|  |  |
|---|---|
| tables read | `scdstates`, `states` |
| tables written | `scdstates` |
| files read | — |
| files written | — |

## Context variables

_None._

## Maps

### `tMap_1` (tMap)

Output `ToSCDTable`:

| column | ← expression |
|---|---|
| `idstate` | `row1.idstate` |
| `labelstate` | `row1.labelstate` |
| `date` | `TalendDate.getDate("CCYY-MM-DD hh:mm:ss")` |


## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `scdstates.date` |  | `TalendDate.getDate("CCYY-MM-DD hh:mm:ss")` |  | 0 |
| `scdstates.labelstate` |  | `states.labelstate` | `states.labelstate` | 0 |
| `scdstates.idstate` |  | `states.idstate` | `states.idstate` | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tMap_1` reads or writes `C:/Talend/Logiciels/TOS/TOS2.4.2/Install/TOS-All-r17347-V2.4.2/workspace/temp`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `tMysqlSCD` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

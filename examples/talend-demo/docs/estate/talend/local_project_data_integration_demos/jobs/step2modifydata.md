---
title: "step2ModifyData"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# step2ModifyData

Folder `t01_Compo/Databases/SCD`, version 0.1. Source: `process/t01_Compo/Databases/SCD/step2ModifyData_0.1.item`.

|  |  |
|---|---|
| tables read | `states` |
| tables written | `states` |
| files read | — |
| files written | — |

## Context variables

_None._

## Maps

### `tMap_1` (tMap)

Output `ToStates`:

| column | ← expression |
|---|---|
| `idstate` | `FromStates.idstate` |
| `labelstate` | `StringHandling.UPCASE(FromStates.labelstate)` |


## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `states.labelstate` | INSERT | `TalendString.getAsciiRandomString(6)` |  | 0 |
| `states.idstate` | DELETE | `states.idstate` | `states.idstate` | 0 |
| `states.idstate` | INSERT | `Integer.decode(globalMap.get("idStatesMax").toString())` |  | 0 |
| `states.idstate` | UPDATE | `states.idstate` | `states.idstate` | 0 |
| `states.labelstate` | DELETE | `states.labelstate` | `states.labelstate` | 0 |
| `states.labelstate` | UPDATE | `StringHandling.UPCASE(states.labelstate)` | `states.labelstate` | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tMap_1` reads or writes `C:/Talend/Logiciels/TOS/TOS2.4.2/Install/TOS-All-r17347-V2.4.2/workspace/temp`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `step2ModifyData` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

---
title: "DirectoryService"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# DirectoryService

Folder `t01_Compo/DataQuality/tFuzzyMatch`, version 0.1. Source: `process/t01_Compo/DataQuality/tFuzzyMatch/DirectoryService_0.1.item`.

|  |  |
|---|---|
| tables read | `directory` |
| tables written | — |
| files read | — |
| files written | — |

## Context variables

_None._

## Maps

### `tMap_1` (tMap)

Output `ToTLogRow`:

| column | ← expression |
|---|---|
| `name` | `directory.name` |
| `firstname` | `directory.firstname` |
| `city` | `directory.city` |
| `phonenumber` | `directory.phonenumber` |

### `tMap_2` (tMap)

Output `ToFuzzy`:

| column | ← expression |
|---|---|
| `City` | `FromPhoneRequest.City` |
| `NameOK` | `FromPhoneRequest.MATCHING` |


## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tMap_2` reads or writes `C:/Talend/Logiciels/TIS/TIS2.4.1/Install/TISEE/TIS_EE-All-r16077-V2.4.1/workspace/temp`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `DirectoryService` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

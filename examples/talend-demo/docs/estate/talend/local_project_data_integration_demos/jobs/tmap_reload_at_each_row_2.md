---
title: "tMap_Reload_At_Each_Row_2"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tMap_Reload_At_Each_Row_2

Folder `t01_Compo/Processing/tMap/Reload_At_Each_Row`, version 0.1. Source: `process/t01_Compo/Processing/tMap/Reload_At_Each_Row/tMap_Reload_At_Each_Row_2_0.1.item`.

|  |  |
|---|---|
| tables read | `customer`, `state` |
| tables written | — |
| files read | — |
| files written | — |

Runs: [tMap_Reload_At_Each_Row_1](../../../talend/local_project_data_integration_demos/jobs/tmap_reload_at_each_row_1.md).

## Context variables

_None._

## Maps

### `tMap_1` (tMap)

Output `ErrorReject`:

| column | ← expression |
|---|---|
| `errorMessage` | — |
| `errorStackTrace` | — |

Output `out`:

| column | ← expression |
|---|---|
| `customername` | `customer.customername` |
| `usstatename` | `state.usstatename` |


## Column lineage

_None._

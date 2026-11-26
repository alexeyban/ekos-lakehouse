---
title: "ELTdemo"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# ELTdemo

Folder `t01_Compo/ELT`, version 0.1. Source: `process/t01_Compo/ELT/ELTdemo_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | `results` |
| files read | — |
| files written | — |

Runs: [createTables4ETL](../../../talend/local_project_data_integration_demos/jobs/createtables4etl.md).

## Context variables

_None._

## Maps

### `tELTMysqlMap_1` (ELT map)

Output `table2`:

| column | ← expression |
|---|---|
| `ID` | `employees.ID` |
| `NAME` | `employees.NAME` |
| `ID_MANAGER` | `employees.ID_MANAGER` |
| `NAME_MANAGER` | `managers.NAME` |


## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `results.NAME` | INSERT | `.NAME` | `.NAME` | 0 |
| `results.ID` | INSERT | `.ID` | `.ID` | 0 |
| `results.ID_MANAGER` | INSERT | `.ID_MANAGER` | `.ID_MANAGER` | 0 |
| `results.NAME_MANAGER` | INSERT | `.NAME` | `.NAME` | 0 |

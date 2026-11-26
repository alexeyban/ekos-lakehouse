---
title: "createTables4ETL"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# createTables4ETL

Folder `t01_Compo/ELT`, version 0.1. Source: `process/t01_Compo/ELT/createTables4ETL_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | `employees` |
| files read | — |
| files written | — |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `employees.NAME` | INSERT | `TalendDataGenerator.getLastName()` |  | 0 |
| `employees.ID_MANAGER` | INSERT | `1,6` |  | 0 |
| `employees.ID` | INSERT | `Numeric.sequence("id",1,1)` |  | 0 |

---
title: "tMysqlBulkExec"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tMysqlBulkExec

Folder `t01_Compo/Databases/Bulk`, version 0.1. Source: `process/t01_Compo/Databases/Bulk/tMysqlBulkExec_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | `demotable` |
| files read | — |
| files written | — |

Runs: [tMysqlOutputBulk](../../../talend/local_project_data_integration_demos/jobs/tmysqloutputbulk.md).

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `demotable.flag` | INSERT | `?(tMysqlBulkExec_1.flag)` |  | 1 |
| `demotable.number` | INSERT | `?(tMysqlBulkExec_1.number)` |  | 1 |
| `demotable.date` | INSERT | `?(tMysqlBulkExec_1.date)` |  | 1 |
| `demotable.txt` | INSERT | `?(tMysqlBulkExec_1.txt)` |  | 1 |

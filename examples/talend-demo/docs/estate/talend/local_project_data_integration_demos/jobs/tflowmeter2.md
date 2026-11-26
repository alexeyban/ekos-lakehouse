---
title: "tFlowMeter2"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFlowMeter2

Folder `t01_Compo/LogError`, version 0.1. Source: `process/t01_Compo/LogError/tFlowMeter2_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | — |
| files written | `{{context.root.concat("talend_files_0.1/out/test.csv")}}` |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `{{context.root.concat("talend_files_0.1/out/test.csv")}}.newColumn2` |  | `TalendDate.getDate("CCYY-MM-DD hh:mm:ss")` |  | 0 |
| `{{context.root.concat("talend_files_0.1/out/test.csv")}}.newColumn1` |  | `TalendDate.getDate("CCYY-MM-DD hh:mm:ss")` |  | 0 |
| `{{context.root.concat("talend_files_0.1/out/test.csv")}}.newColumn` |  | `TalendDate.getDate("CCYY-MM-DD hh:mm:ss")` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tFlowMeter2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

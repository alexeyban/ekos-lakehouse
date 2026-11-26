---
title: "tFileOutputLDIF"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileOutputLDIF

Folder `t01_Compo/File/Output`, version 0.1. Source: `process/t01_Compo/File/Output/tFileOutputLDIF_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | — |
| files written | `{{context.root.concat("talend_files_0.1/out/sample.ldif")}}` |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `{{context.root.concat("talend_files_0.1/out/sample.ldif")}}.newColumn` |  | `TalendString.getAsciiRandomString(3)` |  | 0 |
| `{{context.root.concat("talend_files_0.1/out/sample.ldif")}}.newColumn1` |  | `TalendDataGenerator.getFirstName()` |  | 0 |
| `{{context.root.concat("talend_files_0.1/out/sample.ldif")}}.newColumn2` |  | `TalendDataGenerator.getLastName()` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tFileOutputLDIF` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

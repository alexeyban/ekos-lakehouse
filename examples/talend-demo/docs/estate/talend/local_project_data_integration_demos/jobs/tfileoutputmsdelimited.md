---
title: "tFileOutputMSDelimited"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileOutputMSDelimited

> tFileOutputMSPositional

Folder `t01_Compo/File/Output`, version 0.1. Source: `process/t01_Compo/File/Output/tFileOutputMSDelimited_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | — |
| files written | `{{context.root.concat("talend_files_0.1/out/FileOutputMSDelimited.csv")}}` |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `{{context.root.concat("talend_files_0.1/out/FileOutputMSDelimited.csv")}}.id` |  | `Numeric.sequence("s1",1,1)` |  | 0 |
| `{{context.root.concat("talend_files_0.1/out/FileOutputMSDelimited.csv")}}.pays` |  | `"FRANCE"` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tFileOutputMSDelimited` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

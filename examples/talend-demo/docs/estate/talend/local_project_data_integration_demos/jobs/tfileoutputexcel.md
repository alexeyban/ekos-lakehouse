---
title: "tFileOutputExcel"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileOutputExcel

Folder `t01_Compo/File/Output`, version 0.1. Source: `process/t01_Compo/File/Output/tFileOutputExcel_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | — |
| files written | `{{context.root.concat("talend_files_0.1/out/sample.xls")}}` |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `{{context.root.concat("talend_files_0.1/out/sample.xls")}}.number` |  | `1,2,3,4,5,6,7,8,9,10` |  | 0 |
| `{{context.root.concat("talend_files_0.1/out/sample.xls")}}.flag` |  | `true,false` |  | 0 |
| `{{context.root.concat("talend_files_0.1/out/sample.xls")}}.date` |  | `"2007-01-21", "2007-02-22", "2007-03-23", "2006-04-24", "2006-05-25", "2006-06-26", "2005-07-27", "2005-08-28", "2005-08-29", "2004-11-21"` |  | 0 |
| `{{context.root.concat("talend_files_0.1/out/sample.xls")}}.txt` |  | `"text1", "text2", "text3", "text4", "text5", "text6", "text7", "text8", "text9", "text10"` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tFileOutputExcel` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

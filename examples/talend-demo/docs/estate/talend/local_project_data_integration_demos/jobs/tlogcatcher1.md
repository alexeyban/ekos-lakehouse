---
title: "tLogCatcher1"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tLogCatcher1

Folder `t01_Compo/LogError`, version 0.1. Source: `process/t01_Compo/LogError/tLogCatcher1_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `pathError` |
| files written | `{{context.root.concat("talend_files_0.1/out/test2.csv")}}` |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `{{context.root.concat("talend_files_0.1/out/test2.csv")}}.newColumn` |  | `pathError.newColumn` | `pathError.newColumn` | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **repeated_message** (copy_paste): the message `"tFileInputDelimited have a problem"` is written in 2 jobs (tLogCatcher1, tLogCatcher2); it names `tFileInputDelimited`, yet 2 of them (tLogCatcher1, tLogCatcher2) are not that job and do not call it: probably copied
- **uncalled_job** (dead_item): `tLogCatcher1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

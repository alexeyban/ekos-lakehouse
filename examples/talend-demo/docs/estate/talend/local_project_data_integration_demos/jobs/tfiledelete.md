---
title: "tFileDelete"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileDelete

Folder `t01_Compo/File/Management`, version 0.1. Source: `process/t01_Compo/File/Management/tFileDelete_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | — |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tFileDelete_1` reads or writes `C:/Talend/Logiciels/TOS/TOS2.4.2/Install/TOS-All-r17347-V2.4.2/workspace`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `tFileDelete` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

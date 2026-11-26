---
title: "tPop"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tPop

Folder `t01_Compo/Internet`, version 0.1. Source: `process/t01_Compo/Internet/tPop_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `{{(String)globalMap.get("tPOP_1_CURRENT_FILEPATH")}}` |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tFileInputMail_1` reads or writes `D:/TOS-All-r5798-V2.2.0RC1/workspace/`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `tPop` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

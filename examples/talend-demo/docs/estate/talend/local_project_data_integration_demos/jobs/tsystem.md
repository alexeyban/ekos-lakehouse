---
title: "tSystem"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tSystem

Folder `t01_Compo/System_`, version 0.1. Source: `process/t01_Compo/System_/tSystem_0.1.item`.

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

- **absolute_path** (environment): `tSystem_1` reads or writes `D:/TOS-All-r14257-V2.4.0RC2/workspace`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `tSystem` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

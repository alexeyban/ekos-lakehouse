---
title: "tOracleInput"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tOracleInput

Folder `t01_Compo/Databases/InOut/Oracle`, version 0.1. Source: `process/t01_Compo/Databases/InOut/Oracle/tOracleInput_0.1.item`.

|  |  |
|---|---|
| tables read | `demotable` |
| tables written | — |
| files read | — |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tOracleInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

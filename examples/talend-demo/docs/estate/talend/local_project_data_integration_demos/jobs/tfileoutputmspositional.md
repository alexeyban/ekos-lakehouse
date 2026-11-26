---
title: "tFileOutputMSPositional"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileOutputMSPositional

> tFileOutputMSPositional

Folder `t01_Compo/File/Output`, version 0.1. Source: `process/t01_Compo/File/Output/tFileOutputMSPositional_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | — |
| files written | — |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `.id` |  | `Numeric.sequence("s1",1,1)` |  | 0 |
| `.pays` |  | `"FRANCE"` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tFileOutputMSPositional` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

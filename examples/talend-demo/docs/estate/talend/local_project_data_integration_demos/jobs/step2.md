---
title: "step2"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# step2

Folder `t01_Compo/Orchestration/tWaitForSQL`, version 0.1. Source: `process/t01_Compo/Orchestration/tWaitForSQL/step2_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | `twaitforsqldata` |
| files read | — |
| files written | — |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `twaitforsqldata.date` | INSERT | `"2007-01-21", "2007-02-22", "2007-03-23", "2006-04-24", "2006-05-25", "2006-06-26", "2005-07-27", "2005-08-28", "2005-08-29", "2004-11-21"` |  | 0 |
| `twaitforsqldata.txt` | INSERT | `"text1", "text2", "text3", "text4", "text5", "text6", "text7", "text8", "text9", "text10"` |  | 0 |
| `twaitforsqldata.number` | INSERT | `Numeric.sequence("s1",1,1)` |  | 0 |
| `twaitforsqldata.flag` | INSERT | `true,false` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `step2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

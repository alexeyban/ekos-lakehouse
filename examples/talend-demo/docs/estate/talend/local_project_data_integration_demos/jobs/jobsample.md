---
title: "jobSample"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# jobSample

Folder `t01_Compo/System_/tRunJobRow`, version 0.1. Source: `process/t01_Compo/System_/tRunJobRow/jobSample_0.1.item`.

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
| `.date` |  | `"2007-01-21", "2007-02-22", "2007-03-23", "2006-04-24", "2006-05-25", "2006-06-26", "2005-07-27", "2005-08-28", "2005-08-29", "2004-11-21"` |  | 0 |
| `.txt` |  | `"text1", "text2", "text3", "text4", "text5", "text6", "text7", "text8", "text9", "text10"` |  | 0 |
| `.flag` |  | `true,false` |  | 0 |
| `.number` |  | `1,2,3,4,5,6,7,8,9,10` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `jobSample` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

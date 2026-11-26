---
title: "tMap_3"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tMap_3

Folder `t01_Compo/Processing/tMap`, version 0.1. Source: `process/t01_Compo/Processing/tMap/tMap_3_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | — |
| files written | — |

## Context variables

_None._

## Maps

### `tMap_1` (tMap)

Output `flag_eq_true`:

Filter: `row1.flag`

| column | ← expression |
|---|---|
| `filter` | `"flag = true"` |
| `number` | `row1.number` |
| `txt` | `row1.txt` |
| `date` | `row1.date` |
| `flag` | `row1.flag` |

Output `flaq_ne_true`:

| column | ← expression |
|---|---|
| `filter` | `"flag = false"` |
| `number` | `row1.number` |
| `txt` | `row1.txt` |
| `date` | `row1.date` |
| `flag` | `row1.flag` |


## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tMap_3` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

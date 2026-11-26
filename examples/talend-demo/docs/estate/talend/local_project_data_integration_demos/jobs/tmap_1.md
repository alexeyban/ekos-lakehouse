---
title: "tMap_1"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tMap_1

Folder `t01_Compo/Processing/tMap`, version 0.1. Source: `process/t01_Compo/Processing/tMap/tMap_1_0.1.item`.

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

Output `output`:

| column | ← expression |
|---|---|
| `number` | `row1.number * 10` |
| `txt` | `StringHandling.UPCASE(row1.txt)` |


## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tMap_1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

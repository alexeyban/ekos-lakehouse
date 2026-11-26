---
title: "tMapParse"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tMapParse

Folder `t01_Compo/Processing/tMap`, version 0.1. Source: `process/t01_Compo/Processing/tMap/tMapParse_0.1.item`.

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

Output `newOutput1`:

| column | ← expression |
|---|---|
| `c_Integer` | `row1.c_Integer` |
| `c_string` | `row2.c_string` |


## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tMapParse` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

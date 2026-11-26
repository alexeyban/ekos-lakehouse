---
title: "demo03_tMap"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# demo03_tMap

Folder `t03_DemoRoutines`, version 0.1. Source: `process/t03_DemoRoutines/demo03_tMap_0.1.item`.

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

Output `out1`:

Filter: `DemoRoutines.getFilter(row1.value ,row1.value2)`

| column | ← expression |
|---|---|
| `value` | `row1.value` |
| `value2` | `row1.value2` |
| `label` | `DemoRoutines.getLabel(row1.value)` |


## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `demo03_tMap` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

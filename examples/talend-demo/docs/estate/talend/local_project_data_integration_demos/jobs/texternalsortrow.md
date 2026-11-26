---
title: "tExternalSortRow"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tExternalSortRow

Folder `t01_Compo/Processing`, version 0.1. Source: `process/t01_Compo/Processing/tExternalSortRow_0.1.item`.

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

- **absolute_path** (environment): `tExternalSortRow_1` reads or writes `C:/talend_file/outputSorted.txt`, an absolute path: it ties the job to one machine's layout
- **absolute_path** (environment): `tExternalSortRow_1` reads or writes `C:/GnuWin32/bin/sort.exe`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `tExternalSortRow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

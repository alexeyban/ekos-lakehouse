---
title: "tContextLoad_and_file"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tContextLoad_and_file

Folder `t04_Context`, version 0.1. Source: `process/t04_Context/tContextLoad_and_file_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `C://talend_files/in/config.txt` |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tFileInputDelimited_1` reads or writes `C://talend_files/in/config.txt`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `tContextLoad_and_file` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

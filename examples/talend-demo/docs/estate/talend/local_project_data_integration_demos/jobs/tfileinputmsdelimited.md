---
title: "tFileInputMSDelimited"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileInputMSDelimited

> tFileInputMSDelimited

Folder `t01_Compo/File/Input`, version 0.1. Source: `process/t01_Compo/File/Input/tFileInputMSDelimited_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `C:\TalendDemoFiles\talend_files_0.1/in/FileInput/MSInputDelimited.txt` |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tFileInputMSDelimited_1` reads or writes `C:\TalendDemoFiles\talend_files_0.1/in/FileInput/MSInputDelimited.txt`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `tFileInputMSDelimited` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

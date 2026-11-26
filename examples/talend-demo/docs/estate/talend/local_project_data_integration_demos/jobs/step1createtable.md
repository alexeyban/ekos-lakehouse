---
title: "step1CreateTable"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# step1CreateTable

Folder `t01_Compo/Databases/SCD`, version 0.1. Source: `process/t01_Compo/Databases/SCD/step1CreateTable_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | `states` |
| files read | `C:\TalendDemoFiles\talend_files_0.1/in/state.txt` |
| files written | — |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `states.labelstate` | INSERT | `state.txt.labelstate` | `state.txt.labelstate` | 0 |
| `states.idstate` | INSERT | `state.txt.idstate` | `state.txt.idstate` | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tFileInputDelimited_1` reads or writes `C:\TalendDemoFiles\talend_files_0.1/in/state.txt`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `step1CreateTable` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

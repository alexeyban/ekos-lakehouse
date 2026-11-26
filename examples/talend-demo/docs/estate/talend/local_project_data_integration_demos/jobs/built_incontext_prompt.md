---
title: "built_inContext_prompt"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# built_inContext_prompt

Folder `t04_Context`, version 0.1. Source: `process/t04_Context/built_inContext_prompt_0.1.item`.

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

- **absolute_path** (environment): `tFileFetch_1` reads or writes `C:/talend_files`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `built_inContext_prompt` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

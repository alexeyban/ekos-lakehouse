---
title: "tFileFetch"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileFetch

Folder `t01_Compo/Internet`, version 0.1. Source: `process/t01_Compo/Internet/tFileFetch_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `{{context.root.concat("talend_files_0.1/out/file_fetch.txt")}}` |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tFileFetch` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

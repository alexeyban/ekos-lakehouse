---
title: "tFileRegex"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileRegex

Folder `t01_Compo/File/Input`, version 0.1. Source: `process/t01_Compo/File/Input/tFileRegex_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `{{context.root.concat("talend_files_0.1/in/FileInput/demo_regex.txt")}}` |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tFileRegex` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

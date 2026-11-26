---
title: "tExtractRegexFields"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tExtractRegexFields

Folder `t01_Compo/Processing/Fields`, version 0.1. Source: `process/t01_Compo/Processing/Fields/tExtractRegexFields_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `{{context.root.concat("talend_files_0.1/in/Processing/ExtractRegexFields.txt")}}` |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tExtractRegexFields` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

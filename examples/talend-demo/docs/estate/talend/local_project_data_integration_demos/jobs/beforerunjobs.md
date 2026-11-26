---
title: "beforeRunJobs"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# beforeRunJobs

Folder , version 0.1. Source: `process/beforeRunJobs_0.1.item`.

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

- **absolute_path** (environment): `tFileUnarchive_1` reads or writes `C:\TalendDemoFiles\`, an absolute path: it ties the job to one machine's layout
- **uncalled_job** (dead_item): `beforeRunJobs` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

---
title: "DirectoryGeneration"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# DirectoryGeneration

Folder `t01_Compo/DataQuality/tFuzzyMatch`, version 0.1. Source: `process/t01_Compo/DataQuality/tFuzzyMatch/DirectoryGeneration_0.1.item`.

|  |  |
|---|---|
| tables read | `annuaire` |
| tables written | `directory` |
| files read | — |
| files written | — |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `directory.city` | INSERT | `TalendDataGenerator.getUsCity()` |  | 0 |
| `directory.name` | INSERT | `TalendDataGenerator.getLastName()` |  | 0 |
| `directory.firstname` | INSERT | `TalendDataGenerator.getFirstName()` |  | 0 |
| `directory.phonenumber` | INSERT | `GetRandomPhoneNum.GetPhone()` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `DirectoryGeneration` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

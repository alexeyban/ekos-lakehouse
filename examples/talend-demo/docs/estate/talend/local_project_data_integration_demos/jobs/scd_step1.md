---
title: "scd_step1"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# scd_step1

Folder `t01_Compo/Databases/SCD`, version 0.1. Source: `process/t01_Compo/Databases/SCD/scd_step1_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | `employee` |
| files read | — |
| files written | — |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `employee.id` | INSERT | `Numeric.sequence("s1",1,1)` |  | 0 |
| `employee.name` | INSERT | `TalendDataGenerator.getLastName()` |  | 0 |
| `employee.position` | INSERT | `"Consultant"` |  | 0 |
| `employee.salary` | INSERT | `Numeric.random(1000,2000)` |  | 0 |
| `employee.city` | INSERT | `TalendDataGenerator.getUsCity()` |  | 0 |
| `employee.address` | INSERT | `TalendDataGenerator.getUsStreet()` |  | 0 |
| `employee.firstname` | INSERT | `TalendDataGenerator.getFirstName()` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `scd_step1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

---
title: "scd_step2"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# scd_step2

Folder `t01_Compo/Databases/SCD`, version 0.1. Source: `process/t01_Compo/Databases/SCD/scd_step2_0.1.item`.

|  |  |
|---|---|
| tables read | `employee`, `employee_scd` |
| tables written | `employee`, `employee_scd` |
| files read | — |
| files written | — |

## Context variables

_None._

## Maps

### `tMap_1` (tMap)

Output `employee_scd`:

| column | ← expression |
|---|---|
| `id` | `employee.id` |
| `firstname` | `employee.firstname` |
| `name` | `employee.name` |
| `position` | `employee.position` |
| `address` | `employee.address` |
| `city` | `employee.city` |
| `salary` | `employee.salary` |

### `tMap_3` (tMap)

Output `copyOfemployee_scd`:

| column | ← expression |
|---|---|
| `id` | `row5.id` |
| `firstname` | `row5.firstname` |
| `name` | `row5.name` |
| `position` | `row5.position` |
| `address` | `row5.address` |
| `city` | `row5.city` |
| `salary` | `row5.salary` |


## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `employee_scd.address` |  | `employee.address` | `employee.address` | 0 |
| `employee_scd.previous_salary` |  | `?(tMap_1.previous_salary)` |  | 1 |
| `employee_scd.sequence` |  | `?(tMap_1.sequence)` |  | 1 |
| `employee_scd.name` |  | `employee.name` | `employee.name` | 0 |
| `employee_scd.name` |  | `employee.name` | `employee.name` | 0 |
| `employee_scd.address` |  | `employee.address` | `employee.address` | 0 |
| `employee_scd.sequence` |  | `?(tMap_3.sequence)` |  | 1 |
| `employee_scd.salary` |  | `employee.salary` | `employee.salary` | 0 |
| `employee_scd.version` |  | `?(tMap_3.version)` |  | 1 |
| `employee_scd.start_date` |  | `?(tMap_1.start_date)` |  | 1 |
| `employee_scd.active` |  | `?(tMap_3.active)` |  | 1 |
| `employee_scd.id` |  | `employee.id` | `employee.id` | 0 |
| `employee.salary` | UPDATE_OR_INSERT | `Numeric.random(3000,4000)` |  | 0 |
| `employee_scd.start_date` |  | `?(tMap_3.start_date)` |  | 1 |
| `employee_scd.firstname` |  | `employee.firstname` | `employee.firstname` | 0 |
| `employee_scd.active` |  | `?(tMap_1.active)` |  | 1 |
| `employee_scd.end_date` |  | `?(tMap_1.end_date)` |  | 1 |
| `employee_scd.city` |  | `employee.city` | `employee.city` | 0 |
| `employee_scd.previous_salary` |  | `?(tMap_3.previous_salary)` |  | 1 |
| `employee_scd.firstname` |  | `employee.firstname` | `employee.firstname` | 0 |
| `employee.name` | UPDATE_OR_INSERT | `"ATIGOSSOU"` |  | 0 |
| `employee_scd.end_date` |  | `?(tMap_3.end_date)` |  | 1 |
| `employee_scd.position` |  | `employee.position` | `employee.position` | 0 |
| `employee.position` | UPDATE_OR_INSERT | `"CIO"` |  | 0 |
| `employee_scd.city` |  | `employee.city` | `employee.city` | 0 |
| `employee_scd.salary` |  | `employee.salary` | `employee.salary` | 0 |
| `employee_scd.id` |  | `employee.id` | `employee.id` | 0 |
| `employee.firstname` | UPDATE_OR_INSERT | `"Gilles"` |  | 0 |
| `employee_scd.position` |  | `employee.position` | `employee.position` | 0 |
| `employee.address` | UPDATE_OR_INSERT | `"Palais de l'Elysée"` |  | 0 |
| `employee.id` | UPDATE_OR_INSERT | `1` |  | 0 |
| `employee_scd.version` |  | `?(tMap_1.version)` |  | 1 |
| `employee.city` | UPDATE_OR_INSERT | `"Paris"` |  | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `scd_step2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

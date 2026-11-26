---
title: "generic_elt_step1"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# generic_elt_step1

Folder `t01_Compo/ELT/Generic`, version 0.1. Source: `process/t01_Compo/ELT/Generic/generic_elt_step1_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | `customer`, `state` |
| files read | `{{context.root.concat("talend_files_0.1/in/customer.csv")}}`, `{{context.root.concat("talend_files_0.1/in/state.txt")}}` |
| files written | — |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `customer.CustomerAddress` | INSERT | `customer.csv")}}.CustomerAddress` | `customer.csv")}}.CustomerAddress` | 0 |
| `customer.id` | INSERT | `customer.csv")}}.id` | `customer.csv")}}.id` | 0 |
| `state.labelstate` | INSERT | `state.txt")}}.labelstate` | `state.txt")}}.labelstate` | 0 |
| `customer.id2` | INSERT | `customer.csv")}}.id2` | `customer.csv")}}.id2` | 0 |
| `customer.Somme2` | INSERT | `customer.csv")}}.Somme2` | `customer.csv")}}.Somme2` | 0 |
| `customer.Somme5` | INSERT | `customer.csv")}}.Somme5` | `customer.csv")}}.Somme5` | 0 |
| `customer.Somme7` | INSERT | `customer.csv")}}.Somme7` | `customer.csv")}}.Somme7` | 0 |
| `customer.Somme3` | INSERT | `customer.csv")}}.Somme3` | `customer.csv")}}.Somme3` | 0 |
| `state.idstate` | INSERT | `state.txt")}}.idstate` | `state.txt")}}.idstate` | 0 |
| `customer.Somme1` | INSERT | `customer.csv")}}.Somme1` | `customer.csv")}}.Somme1` | 0 |
| `customer.idState` | INSERT | `customer.csv")}}.idState` | `customer.csv")}}.idState` | 0 |
| `customer.RegTime` | INSERT | `customer.csv")}}.RegTime` | `customer.csv")}}.RegTime` | 0 |
| `customer.CustomerName` | INSERT | `customer.csv")}}.CustomerName` | `customer.csv")}}.CustomerName` | 0 |
| `customer.RegisterTime` | INSERT | `customer.csv")}}.RegisterTime` | `customer.csv")}}.RegisterTime` | 0 |
| `customer.Somme6` | INSERT | `customer.csv")}}.Somme6` | `customer.csv")}}.Somme6` | 0 |
| `customer.Somme4` | INSERT | `customer.csv")}}.Somme4` | `customer.csv")}}.Somme4` | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `generic_elt_step1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

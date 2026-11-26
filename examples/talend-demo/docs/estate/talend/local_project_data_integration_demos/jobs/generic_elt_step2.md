---
title: "generic_elt_step2"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# generic_elt_step2

Folder `t01_Compo/ELT/Generic`, version 0.1. Source: `process/t01_Compo/ELT/Generic/generic_elt_step2_0.1.item`.

|  |  |
|---|---|
| tables read | `customer`, `state` |
| tables written | `aggregation` |
| files read | — |
| files written | — |

## Context variables

_None._

## Maps

### `tELTMysqlMap_1` (ELT map)

Output `table1`:

| column | ← expression |
|---|---|
| `id` | `customer.idcustomer` |
| `CustomerName` | `UPPER(customer.customername)` |
| `idusstate` | `customer.idusstate` |
| `Flag` | `if(customer.idcustomer<2500,"<2500",">=2500")` |


## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `aggregation.CustomerName` | INSERT | `UPPER(customer.customername)` | `customer.customername` | 0 |
| `aggregation.id` | INSERT | `customer.idcustomer` | `customer.idcustomer` | 0 |
| `aggregation.idusstate` | INSERT | `customer.idusstate` | `customer.idusstate` | 0 |
| `aggregation.Flag` | INSERT | `if(customer.idcustomer<2500,"<2500",">=2500")` | `customer.idcustomer` | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `generic_elt_step2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

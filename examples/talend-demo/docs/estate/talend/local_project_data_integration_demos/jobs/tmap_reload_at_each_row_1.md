---
title: "tMap_Reload_At_Each_Row_1"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tMap_Reload_At_Each_Row_1

Folder `t01_Compo/Processing/tMap/Reload_At_Each_Row`, version 0.1. Source: `process/t01_Compo/Processing/tMap/Reload_At_Each_Row/tMap_Reload_At_Each_Row_1_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | `customer`, `state` |
| files read | — |
| files written | — |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `state.idusstate` | INSERT | `Numeric.sequence("s2",1,1)` |  | 0 |
| `customer.idcustomer` | INSERT | `Numeric.sequence("s1",1,1)` |  | 0 |
| `customer.customername` | INSERT | `TalendDataGenerator.getLastName()` |  | 0 |
| `customer.idusstate` | INSERT | `Numeric.random(1,50)` |  | 0 |
| `state.usstatename` | INSERT | `TalendDataGenerator.getUsState()` |  | 0 |

---
title: "xmlSample2"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# xmlSample2

Folder `t01_Compo/XML`, version 0.1. Source: `process/t01_Compo/XML/xmlSample2_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `{{context.root.concat("talend_files_0.1/in/XML/sample.xml")}}` |
| files written | — |

## Context variables

_None._

## Maps

### `tMap_1` (tMap)

Output `output`:

| column | ← expression |
|---|---|
| `titre` | `row1.idtitre` |
| `sousTitre` | `row1.sousTitre` |
| `editeur` | `row1.editeur` |
| `auteur` | `row1.auteur` |
| `name` | `row3.name` |
| `Paysname` | `row3.Paysname` |
| `stateName` | `row3.stateName` |


## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tFileInputXML_3` reads or writes `/home/nfrancisco/Bureau/TOS-All-r39316-V4.0.0RC3/workspace/temp.xml`, an absolute path inside a person's own folders: it exists on one machine
- **uncalled_job** (dead_item): `xmlSample2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

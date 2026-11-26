---
title: "tFileInputXML"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileInputXML

Folder `t01_Compo/File/Input`, version 0.1. Source: `process/t01_Compo/File/Input/tFileInputXML_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `{{context.root.concat("talend_files_0.1/in/FileInput/sample0.xml")}}` |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tFileInputXML_1` reads or writes `/home/nfrancisco/Bureau/TOS-All-r39316-V4.0.0RC3/workspace/temp.xml`, an absolute path inside a person's own folders: it exists on one machine
- **uncalled_job** (dead_item): `tFileInputXML` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

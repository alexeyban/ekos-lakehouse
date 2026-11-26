---
title: "tFileInputMSXML"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileInputMSXML

Folder `t01_Compo/File/Input`, version 0.1. Source: `process/t01_Compo/File/Input/tFileInputMSXML_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `/home/nfrancisco/Talend/talend_files_0.1/in/XML/sample.xml`, `{{context.root.concat("talend_files_0.1/in/XML/sample.xml")}}` |
| files written | — |

## Context variables

_None._

## Column lineage

_None._

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **absolute_path** (environment): `tFileInputXML_1` reads or writes `/home/nfrancisco/Talend/talend_files_0.1/in/XML/sample.xml`, an absolute path inside a person's own folders: it exists on one machine
- **absolute_path** (environment): `tFileInputXML_1` reads or writes `/home/nfrancisco/Talend/Logiciels/TOS/4.0.0M3/Install/TOS-All-r37054-V4.0.0M3/workspace/temp.xml`, an absolute path inside a person's own folders: it exists on one machine
- **uncalled_job** (dead_item): `tFileInputMSXML` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

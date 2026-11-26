---
title: "tFileOutputMSXML"
order: 20
ekos_generated: true
ekos_estate_version: 3
---

# tFileOutputMSXML

> tFileOutputMSXML

Folder `t01_Compo/File/Output`, version 0.1. Source: `process/t01_Compo/File/Output/tFileOutputMSXML_0.1.item`.

|  |  |
|---|---|
| tables read | — |
| tables written | — |
| files read | `{{context.root.concat("talend_files_0.1/in/FileInput/command_details.csv")}}`, `{{context.root.concat("talend_files_0.1/in/FileInput/command_header.csv")}}` |
| files written | `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}` |

## Context variables

_None._

## Column lineage

| target | action | formula | origins | gaps |
|---|---|---|---|---|
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Date_cmd` |  | `command_header.csv")}}.Date_cmd` | `command_details.csv")}}.Date_cmd`, `command_header.csv")}}.Date_cmd` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Telephone` |  | `command_header.csv")}}.Telephone` | `command_details.csv")}}.Telephone`, `command_header.csv")}}.Telephone` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Garantie_opt` |  | `command_header.csv")}}.Garantie_opt` | `command_details.csv")}}.Garantie_opt`, `command_header.csv")}}.Garantie_opt` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Montant_frais_1` |  | `command_header.csv")}}.Montant_frais_1` | `command_details.csv")}}.Montant_frais_1`, `command_header.csv")}}.Montant_frais_1` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Code_expedition` |  | `command_header.csv")}}.Code_expedition` | `command_details.csv")}}.Code_expedition`, `command_header.csv")}}.Code_expedition` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Transaction` |  | `command_header.csv")}}.Transaction` | `command_details.csv")}}.Transaction`, `command_header.csv")}}.Transaction` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Action_comm` |  | `command_header.csv")}}.Action_comm` | `command_details.csv")}}.Action_comm`, `command_header.csv")}}.Action_comm` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Code_etat` |  | `command_header.csv")}}.Code_etat` | `command_details.csv")}}.Code_etat`, `command_header.csv")}}.Code_etat` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Code_frais_5` |  | `command_header.csv")}}.Code_frais_5` | `command_details.csv")}}.Code_frais_5`, `command_header.csv")}}.Code_frais_5` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Nbr_jour` |  | `command_header.csv")}}.Nbr_jour` | `command_details.csv")}}.Nbr_jour`, `command_header.csv")}}.Nbr_jour` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Nom_voie` |  | `command_header.csv")}}.Nom_voie` | `command_details.csv")}}.Nom_voie`, `command_header.csv")}}.Nom_voie` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Adresse_suite` |  | `command_header.csv")}}.Adresse_suite` | `command_details.csv")}}.Adresse_suite`, `command_header.csv")}}.Adresse_suite` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Resp_vente` |  | `command_header.csv")}}.Resp_vente` | `command_details.csv")}}.Resp_vente`, `command_header.csv")}}.Resp_vente` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Form_juridique` |  | `command_header.csv")}}.Form_juridique` | `command_details.csv")}}.Form_juridique`, `command_header.csv")}}.Form_juridique` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Adresse` |  | `command_header.csv")}}.Adresse` | `command_details.csv")}}.Adresse`, `command_header.csv")}}.Adresse` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Ref_client` |  | `command_header.csv")}}.Ref_client` | `command_details.csv")}}.Ref_client`, `command_header.csv")}}.Ref_client` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Bis_ter` |  | `command_header.csv")}}.Bis_ter` | `command_details.csv")}}.Bis_ter`, `command_header.csv")}}.Bis_ter` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Montant_frais_4` |  | `command_header.csv")}}.Montant_frais_4` | `command_details.csv")}}.Montant_frais_4`, `command_header.csv")}}.Montant_frais_4` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Ville` |  | `command_header.csv")}}.Ville` | `command_details.csv")}}.Ville`, `command_header.csv")}}.Ville` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Type_cmd` |  | `command_header.csv")}}.Type_cmd` | `command_details.csv")}}.Type_cmd`, `command_header.csv")}}.Type_cmd` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Code_frais_1` |  | `command_header.csv")}}.Code_frais_1` | `command_details.csv")}}.Code_frais_1`, `command_header.csv")}}.Code_frais_1` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Pays` |  | `command_header.csv")}}.Pays` | `command_details.csv")}}.Pays`, `command_header.csv")}}.Pays` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Code_frais_4` |  | `command_header.csv")}}.Code_frais_4` | `command_details.csv")}}.Code_frais_4`, `command_header.csv")}}.Code_frais_4` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Cond_rglt` |  | `command_header.csv")}}.Cond_rglt` | `command_details.csv")}}.Cond_rglt`, `command_header.csv")}}.Cond_rglt` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Client_livre` |  | `command_header.csv")}}.Client_livre` | `command_details.csv")}}.Client_livre`, `command_header.csv")}}.Client_livre` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.N_Odre` |  | `command_header.csv")}}.N_Odre` | `command_details.csv")}}.N_Odre`, `command_header.csv")}}.N_Odre` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Montant_frais_2` |  | `command_header.csv")}}.Montant_frais_2` | `command_details.csv")}}.Montant_frais_2`, `command_header.csv")}}.Montant_frais_2` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Code_transporteur` |  | `command_header.csv")}}.Code_transporteur` | `command_details.csv")}}.Code_transporteur`, `command_header.csv")}}.Code_transporteur` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Nom` |  | `command_header.csv")}}.Nom` | `command_details.csv")}}.Nom`, `command_header.csv")}}.Nom` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Type_voie` |  | `command_header.csv")}}.Type_voie` | `command_details.csv")}}.Type_voie`, `command_header.csv")}}.Type_voie` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.N_voie` |  | `command_header.csv")}}.N_voie` | `command_details.csv")}}.N_voie`, `command_header.csv")}}.N_voie` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Date_echeance` |  | `command_header.csv")}}.Date_echeance` | `command_details.csv")}}.Date_echeance`, `command_header.csv")}}.Date_echeance` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Montant_frais_3` |  | `command_header.csv")}}.Montant_frais_3` | `command_details.csv")}}.Montant_frais_3`, `command_header.csv")}}.Montant_frais_3` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Code_postal` |  | `command_header.csv")}}.Code_postal` | `command_details.csv")}}.Code_postal`, `command_header.csv")}}.Code_postal` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Code_frais_3` |  | `command_header.csv")}}.Code_frais_3` | `command_details.csv")}}.Code_frais_3`, `command_header.csv")}}.Code_frais_3` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Montant_frais_5` |  | `command_header.csv")}}.Montant_frais_5` | `command_details.csv")}}.Montant_frais_5`, `command_header.csv")}}.Montant_frais_5` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Mode_rglt` |  | `command_header.csv")}}.Mode_rglt` | `command_details.csv")}}.Mode_rglt`, `command_header.csv")}}.Mode_rglt` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Prenom` |  | `command_header.csv")}}.Prenom` | `command_details.csv")}}.Prenom`, `command_header.csv")}}.Prenom` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Client_facture` |  | `command_header.csv")}}.Client_facture` | `command_details.csv")}}.Client_facture`, `command_header.csv")}}.Client_facture` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Livraison_partiel` |  | `command_header.csv")}}.Livraison_partiel` | `command_details.csv")}}.Livraison_partiel`, `command_header.csv")}}.Livraison_partiel` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Code_frais_2` |  | `command_header.csv")}}.Code_frais_2` | `command_details.csv")}}.Code_frais_2`, `command_header.csv")}}.Code_frais_2` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Client` |  | `command_header.csv")}}.Client` | `command_details.csv")}}.Client`, `command_header.csv")}}.Client` | 0 |
| `{{context.root.concat("talend_files_0.1/out/command_MS.xml")}}.Origine_vente` |  | `command_header.csv")}}.Origine_vente` | `command_details.csv")}}.Origine_vente`, `command_header.csv")}}.Origine_vente` | 0 |

## Findings to review

Hypotheses from the assessment, not defects until a person says so.

- **uncalled_job** (dead_item): `tFileOutputMSXML` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all

---
title: "LOCAL_PROJECT_DATA_INTEGRATION_DEMOS: migration assessment"
order: 90
ekos_generated: true
ekos_estate_version: 3
---

# LOCAL_PROJECT_DATA_INTEGRATION_DEMOS: migration assessment

Built from the compiled project, with no model call. Every finding is a hypothesis with its evidence: a person decides whether it is a defect. There is no estimate in days: EKOS has no measured basis for one.

## Findings

### copy paste (1)

- **repeated_message** `message:tfileinputdelimitedhaveaproblem` — the message `"tFileInputDelimited have a problem"` is written in 2 jobs (tLogCatcher1, tLogCatcher2); it names `tFileInputDelimited`, yet 2 of them (tLogCatcher1, tLogCatcher2) are not that job and do not call it: probably copied (process/t01_Compo/LogError/tLogCatcher1_0.1.item:185, process/t01_Compo/LogError/tLogCatcher2_0.1.item:198)

### dead item (158)

- **uncalled_job** `ComponentRow` — `ComponentRow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/ComponentRow_0.1.item)
- **uncalled_job** `Connection` — `Connection` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/Connection_0.1.item)
- **uncalled_job** `DirectoryGeneration` — `DirectoryGeneration` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/DataQuality/tFuzzyMatch/DirectoryGeneration_0.1.item)
- **uncalled_job** `DirectoryService` — `DirectoryService` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/DataQuality/tFuzzyMatch/DirectoryService_0.1.item)
- **uncalled_job** `OnSubjobError` — `OnSubjobError` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t02_Concatening/OnSubjobError_0.1.item)
- **uncalled_job** `beforeRunJobs` — `beforeRunJobs` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/beforeRunJobs_0.1.item)
- **uncalled_job** `built_inContext_1` — `built_inContext_1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t04_Context/built_inContext_1_0.1.item)
- **uncalled_job** `built_inContext_prompt` — `built_inContext_prompt` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t04_Context/built_inContext_prompt_0.1.item)
- **uncalled_job** `demo01_tJava` — `demo01_tJava` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t03_DemoRoutines/demo01_tJava_0.1.item)
- **uncalled_job** `demo02_defineProperties` — `demo02_defineProperties` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t03_DemoRoutines/demo02_defineProperties_0.1.item)
- **uncalled_job** `demo03_tMap` — `demo03_tMap` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t03_DemoRoutines/demo03_tMap_0.1.item)
- **uncalled_job** `demo04_tRowGenerator` — `demo04_tRowGenerator` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t03_DemoRoutines/demo04_tRowGenerator_0.1.item)
- **uncalled_job** `generic_elt_step1` — `generic_elt_step1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/ELT/Generic/generic_elt_step1_0.1.item)
- **uncalled_job** `generic_elt_step2` — `generic_elt_step2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/ELT/Generic/generic_elt_step2_0.1.item)
- **uncalled_job** `jobSample` — `jobSample` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/System_/tRunJobRow/jobSample_0.1.item)
- **uncalled_job** `link_if` — `link_if` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t02_Concatening/link_if_0.1.item)
- **uncalled_job** `link_with_tDie_tWarn` — `link_with_tDie_tWarn` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t02_Concatening/link_with_tDie_tWarn_0.1.item)
- **uncalled_job** `onComponentError` — `onComponentError` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t02_Concatening/onComponentError_0.1.item)
- **uncalled_job** `priorTest` — `priorTest` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/priorTest_0.1.item)
- **uncalled_job** `readme` — `readme` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/readme_0.1.item)
- **uncalled_job** `repositoryContext` — `repositoryContext` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t04_Context/repositoryContext_0.1.item)
- **uncalled_job** `scd_step1` — `scd_step1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/SCD/scd_step1_0.1.item)
- **uncalled_job** `scd_step2` — `scd_step2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/SCD/scd_step2_0.1.item)
- **uncalled_job** `step1CreateTable` — `step1CreateTable` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/SCD/step1CreateTable_0.1.item)
- **uncalled_job** `step2` — `step2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Orchestration/tWaitForSQL/step2_0.1.item)
- **uncalled_job** `step2ModifyData` — `step2ModifyData` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/SCD/step2ModifyData_0.1.item)
- **uncalled_job** `tAS400Input` — `tAS400Input` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/AS400/tAS400Input_0.1.item)
- **uncalled_job** `tAS400Output` — `tAS400Output` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/AS400/tAS400Output_0.1.item)
- **uncalled_job** `tAccessInput` — `tAccessInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/Access/tAccessInput_0.1.item)
- **uncalled_job** `tAccessOutput` — `tAccessOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/Access/tAccessOutput_0.1.item)
- **uncalled_job** `tAddCRC` — `tAddCRC` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/DataQuality/tAddCRC_0.2.item)
- **uncalled_job** `tAddLocationFromIP` — `tAddLocationFromIP` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tAddLocationFromIP_0.1.item)
- **uncalled_job** `tAdvancedFOXML` — `tAdvancedFOXML` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/XML/tAdvancedFOXML_0.1.item)
- **uncalled_job** `tAdvancedFileOutputXML` — `tAdvancedFileOutputXML` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tAdvancedFileOutputXML_0.1.item)
- **uncalled_job** `tAggregateRow` — `tAggregateRow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tAggregateRow_0.1.item)
- **uncalled_job** `tAggregateSortedRow` — `tAggregateSortedRow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tAggregateSortedRow_0.1.item)
- **uncalled_job** `tAssert` — `tAssert` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/LogError/tAssert_0.1.item)
- **uncalled_job** `tChronometer` — `tChronometer` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/LogError/tChronometer_0.1.item)
- **uncalled_job** `tContextLoad` — `tContextLoad` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tContextLoad_0.1.item)
- **uncalled_job** `tContextLoad_and_file` — `tContextLoad_and_file` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t04_Context/tContextLoad_and_file_0.1.item)
- **uncalled_job** `tCreateTemporaryFile` — `tCreateTemporaryFile` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tCreateTemporaryFile_0.1.item)
- **uncalled_job** `tDB2Input` — `tDB2Input` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/DB2/tDB2Input_0.1.item)
- **uncalled_job** `tDB2Output` — `tDB2Output` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/DB2/tDB2Output_0.1.item)
- **uncalled_job** `tDBInput` — `tDBInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/DBGeneric/tDBInput_0.1.item)
- **uncalled_job** `tDBOutput` — `tDBOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/DBGeneric/tDBOutput_0.1.item)
- **uncalled_job** `tDTDValidator` — `tDTDValidator` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/XML/tDTDValidator_0.1.item)
- **uncalled_job** `tDenormalize` — `tDenormalize` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tDenormalize_0.1.item)
- **uncalled_job** `tExternalSortRow` — `tExternalSortRow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tExternalSortRow_0.1.item)
- **uncalled_job** `tExtractDelimitedFields` — `tExtractDelimitedFields` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/Fields/tExtractDelimitedFields_0.1.item)
- **uncalled_job** `tExtractPositionalFields` — `tExtractPositionalFields` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/Fields/tExtractPositionalFields_0.1.item)
- **uncalled_job** `tExtractRegexFields` — `tExtractRegexFields` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/Fields/tExtractRegexFields_0.1.item)
- **uncalled_job** `tFileArchive` — `tFileArchive` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tFileArchive_0.1.item)
- **uncalled_job** `tFileCompare` — `tFileCompare` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tFileCompare_0.1.item)
- **uncalled_job** `tFileDelete` — `tFileDelete` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tFileDelete_0.1.item)
- **uncalled_job** `tFileExist` — `tFileExist` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tFileExist_0.1.item)
- **uncalled_job** `tFileFetch` — `tFileFetch` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Internet/tFileFetch_0.1.item)
- **uncalled_job** `tFileInputARFF` — `tFileInputARFF` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputARFF_0.1.item)
- **uncalled_job** `tFileInputDelimited` — `tFileInputDelimited` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputDelimited_0.1.item)
- **uncalled_job** `tFileInputDelimitedCSV` — `tFileInputDelimitedCSV` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputDelimitedCSV_0.1.item)
- **uncalled_job** `tFileInputExcel` — `tFileInputExcel` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputExcel_0.1.item)
- **uncalled_job** `tFileInputLDIF` — `tFileInputLDIF` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputLDIF_0.1.item)
- **uncalled_job** `tFileInputMSDelimited` — `tFileInputMSDelimited` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputMSDelimited_0.1.item)
- **uncalled_job** `tFileInputMSPositional` — `tFileInputMSPositional` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputMSPositional_0.1.item)
- **uncalled_job** `tFileInputMSXML` — `tFileInputMSXML` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputMSXML_0.1.item)
- **uncalled_job** `tFileInputMail` — `tFileInputMail` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputMail_0.1.item)
- **uncalled_job** `tFileInputPositional` — `tFileInputPositional` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputPositional_0.1.item)
- **uncalled_job** `tFileInputXML` — `tFileInputXML` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileInputXML_0.1.item)
- **uncalled_job** `tFileList` — `tFileList` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tFileList_0.1.item)
- **uncalled_job** `tFileOutputDelimited` — `tFileOutputDelimited` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tFileOutputDelimited_0.1.item)
- **uncalled_job** `tFileOutputDelimitedCSV` — `tFileOutputDelimitedCSV` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tFileOutputDelimitedCSV_0.1.item)
- **uncalled_job** `tFileOutputExcel` — `tFileOutputExcel` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tFileOutputExcel_0.1.item)
- **uncalled_job** `tFileOutputLDIF` — `tFileOutputLDIF` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tFileOutputLDIF_0.1.item)
- **uncalled_job** `tFileOutputMSDelimited` — `tFileOutputMSDelimited` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tFileOutputMSDelimited_0.1.item)
- **uncalled_job** `tFileOutputMSPositional` — `tFileOutputMSPositional` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tFileOutputMSPositional_0.1.item)
- **uncalled_job** `tFileOutputMSXML` — `tFileOutputMSXML` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tFileOutputMSXML_0.1.item)
- **uncalled_job** `tFileOutputPositional` — `tFileOutputPositional` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tFileOutputPositional_0.1.item)
- **uncalled_job** `tFileOutputXML` — `tFileOutputXML` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Output/tFileOutputXML_0.1.item)
- **uncalled_job** `tFileProperties` — `tFileProperties` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tFileProperties_0.1.item)
- **uncalled_job** `tFileRegex` — `tFileRegex` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Input/tFileRegex_0.1.item)
- **uncalled_job** `tFileRowCount` — `tFileRowCount` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tFileRowCount_0.1.item)
- **uncalled_job** `tFileTouch` — `tFileTouch` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tFileTouch_0.1.item)
- **uncalled_job** `tFileUnarchive` — `tFileUnarchive` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/File/Management/tFileUnarchive_0.1.item)
- **uncalled_job** `tFilterColumns` — `tFilterColumns` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tFilterColumns_0.1.item)
- **uncalled_job** `tFilterRows` — `tFilterRows` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tFilterRows_0.1.item)
- **uncalled_job** `tFixedFlowInput` — `tFixedFlowInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tFixedFlowInput_0.1.item)
- **uncalled_job** `tFlowMeter1` — `tFlowMeter1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/LogError/tFlowMeter1_0.1.item)
- **uncalled_job** `tFlowMeter2` — `tFlowMeter2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/LogError/tFlowMeter2_0.1.item)
- **uncalled_job** `tFlowToIterate` — `tFlowToIterate` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tFlowToIterate_0.1.item)
- **uncalled_job** `tForeach` — `tForeach` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tForeach_0.1.item)
- **uncalled_job** `tFuzzyMatch` — `tFuzzyMatch` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/DataQuality/tFuzzyMatch_0.1.item)
- **uncalled_job** `tGroovy` — `tGroovy` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/CustomCode/tGroovy_0.1.item)
- **uncalled_job** `tGroovyFile` — `tGroovyFile` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/CustomCode/tGroovyFile_0.1.item)
- **uncalled_job** `tIntervalMatch` — `tIntervalMatch` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/DataQuality/tIntervalMatch_0.1.item)
- **uncalled_job** `tIterateToFlow` — `tIterateToFlow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Orchestration/tIterateToFlow_0.1.item)
- **uncalled_job** `tJava` — `tJava` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/CustomCode/tJava_0.1.item)
- **uncalled_job** `tJavaFlex` — `tJavaFlex` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/CustomCode/tJavaFlex_0.1.item)
- **uncalled_job** `tJavaRow` — `tJavaRow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/CustomCode/tJavaRow_0.1.item)
- **uncalled_job** `tJoin` — `tJoin` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tJoin_0.1.item)
- **uncalled_job** `tLibraryLoad` — `tLibraryLoad` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/CustomCode/tLibraryLoad_0.1.item)
- **uncalled_job** `tLogCatcher1` — `tLogCatcher1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/LogError/tLogCatcher1_0.1.item)
- **uncalled_job** `tLogCatcher2` — `tLogCatcher2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/LogError/tLogCatcher2_0.1.item)
- **uncalled_job** `tLoop` — `tLoop` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tLoop_0.1.item)
- **uncalled_job** `tMSSqlInput` — `tMSSqlInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/MSSQLServer/tMSSqlInput_0.1.item)
- **uncalled_job** `tMSSqlOutput` — `tMSSqlOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/MSSQLServer/tMSSqlOutput_0.1.item)
- **uncalled_job** `tMapParse` — `tMapParse` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tMap/tMapParse_0.1.item)
- **uncalled_job** `tMap_1` — `tMap_1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tMap/tMap_1_0.1.item)
- **uncalled_job** `tMap_2` — `tMap_2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tMap/tMap_2_0.1.item)
- **uncalled_job** `tMap_3` — `tMap_3` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tMap/tMap_3_0.1.item)
- **uncalled_job** `tMap_4` — `tMap_4` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tMap/tMap_4_0.1.item)
- **uncalled_job** `tMomInput` — `tMomInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Internet/tMomInput_0.1.item)
- **uncalled_job** `tMomOutput` — `tMomOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Internet/tMomOutput_0.1.item)
- **uncalled_job** `tMsgBox` — `tMsgBox` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tMsgBox_0.1.item)
- **uncalled_job** `tMsgBox2` — `tMsgBox2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tMsgBox2_0.1.item)
- **uncalled_job** `tMysqlInput` — `tMysqlInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/MySQL/tMysqlInput_0.1.item)
- **uncalled_job** `tMysqlOutput` — `tMysqlOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/MySQL/tMysqlOutput_0.1.item)
- **uncalled_job** `tMysqlOutputBulkExec` — `tMysqlOutputBulkExec` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/Bulk/tMysqlOutputBulkExec_0.1.item)
- **uncalled_job** `tMysqlSCD` — `tMysqlSCD` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/SCD/tMysqlSCD_0.1.item)
- **uncalled_job** `tMysqlSP` — `tMysqlSP` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/SP/tMysqlSP_0.1.item)
- **uncalled_job** `tNormalize` — `tNormalize` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tNormalize_0.1.item)
- **uncalled_job** `tOracleInput` — `tOracleInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/Oracle/tOracleInput_0.1.item)
- **uncalled_job** `tOracleOutput` — `tOracleOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/Oracle/tOracleOutput_0.1.item)
- **uncalled_job** `tPop` — `tPop` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Internet/tPop_0.1.item)
- **uncalled_job** `tPostgreSQLInput` — `tPostgreSQLInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/PostgreSQL/tPostgreSQLInput_0.1.item)
- **uncalled_job** `tPostgreSQLOutput` — `tPostgreSQLOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/PostgreSQL/tPostgreSQLOutput_0.1.item)
- **uncalled_job** `tPreJob` — `tPreJob` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Orchestration/tPreJob_0.1.item)
- **uncalled_job** `tReplace` — `tReplace` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tReplace_0.1.item)
- **uncalled_job** `tReplaceList` — `tReplaceList` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/DataQuality/tReplaceList_0.1.item)
- **uncalled_job** `tReplicate` — `tReplicate` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tReplicate_0.1.item)
- **uncalled_job** `tRowGenerator` — `tRowGenerator` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tRowGenerator_0.1.item)
- **uncalled_job** `tSQLiteInput` — `tSQLiteInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/SQLite/tSQLiteInput_0.1.item)
- **uncalled_job** `tSQLiteOutput` — `tSQLiteOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/SQLite/tSQLiteOutput_0.1.item)
- **uncalled_job** `tSSH` — `tSSH` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/System_/tSSH_0.1.item)
- **uncalled_job** `tSVNLogInput` — `tSVNLogInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Internet/tSVNLogInput_0.1.item)
- **uncalled_job** `tSampleRow` — `tSampleRow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tSampleRow_0.1.item)
- **uncalled_job** `tSchemaComplianceCheck` — `tSchemaComplianceCheck` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/DataQuality/tSchemaComplianceCheck_0.1.item)
- **uncalled_job** `tSendMail` — `tSendMail` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Internet/tSendMail_0.1.item)
- **uncalled_job** `tServerAlive` — `tServerAlive` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tServerAlive_0.1.item)
- **uncalled_job** `tSetEnv` — `tSetEnv` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/System_/tSetEnv_0.1.item)
- **uncalled_job** `tSetGlobalVar` — `tSetGlobalVar` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/CustomCode/tSetGlobalVar_0.1.item)
- **uncalled_job** `tSleep` — `tSleep` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tSleep_0.1.item)
- **uncalled_job** `tSocketInput` — `tSocketInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Internet/tSocketInput_0.1.item)
- **uncalled_job** `tSocketOutput` — `tSocketOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Internet/tSocketOutput_0.1.item)
- **uncalled_job** `tSortRow` — `tSortRow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Processing/tSortRow_0.1.item)
- **uncalled_job** `tStatCatcher1` — `tStatCatcher1` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/LogError/tStatCatcher1_0.1.item)
- **uncalled_job** `tStatCatcher2` — `tStatCatcher2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/LogError/tStatCatcher2_0.1.item)
- **uncalled_job** `tSybaseInput` — `tSybaseInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/Sybase/tSybaseInput_0.1.item)
- **uncalled_job** `tSybaseOutput` — `tSybaseOutput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Databases/InOut/Sybase/tSybaseOutput_0.1.item)
- **uncalled_job** `tSystem` — `tSystem` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/System_/tSystem_0.1.item)
- **uncalled_job** `tUniqRow` — `tUniqRow` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/DataQuality/tUniqRow_0.1.item)
- **uncalled_job** `tUnite` — `tUnite` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Orchestration/tUnite_0.1.item)
- **uncalled_job** `tWaitForFile` — `tWaitForFile` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Misc/tWaitForFile_0.1.item)
- **uncalled_job** `tWaitForSocket` — `tWaitForSocket` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Orchestration/tWaitForSocket_0.1.item)
- **uncalled_job** `tWaitForSqlData` — `tWaitForSqlData` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Orchestration/tWaitForSQL/tWaitForSqlData_0.1.item)
- **uncalled_job** `tWebServiceInput` — `tWebServiceInput` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/Internet/tWebServiceInput_0.1.item)
- **uncalled_job** `tXSDValidator` — `tXSDValidator` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/XML/tXSDValidator_0.1.item)
- **uncalled_job** `tXSLT` — `tXSLT` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/XML/tXSLT_0.1.item)
- **uncalled_job** `xmlSample2` — `xmlSample2` is not called by any job and calls none: run on its own, by a schedule outside the project, or not at all (process/t01_Compo/XML/xmlSample2_0.1.item)
- **unused_item** `TalendContext:globalContext` — the context `globalContext` is not used by any job (context/globalContext_0.1.item)

### environment (19)

- **absolute_path** `DirectoryService.tMap_2.TEMPORARY_DATA_DIRECTORY` — `tMap_2` reads or writes `C:/Talend/Logiciels/TIS/TIS2.4.1/Install/TISEE/TIS_EE-All-r16077-V2.4.1/workspace/temp`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/DataQuality/tFuzzyMatch/DirectoryService_0.1.item:180)
- **absolute_path** `beforeRunJobs.tFileUnarchive_1.DIRECTORY` — `tFileUnarchive_1` reads or writes `C:\TalendDemoFiles\`, an absolute path: it ties the job to one machine's layout (process/beforeRunJobs_0.1.item:44)
- **absolute_path** `built_inContext_1.tFileFetch_1.DIRECTORY` — `tFileFetch_1` reads or writes `C:/talend_files`, an absolute path: it ties the job to one machine's layout (process/t04_Context/built_inContext_1_0.1.item:89)
- **absolute_path** `built_inContext_prompt.tFileFetch_1.DIRECTORY` — `tFileFetch_1` reads or writes `C:/talend_files`, an absolute path: it ties the job to one machine's layout (process/t04_Context/built_inContext_prompt_0.1.item:84)
- **absolute_path** `demo02_defineProperties.tFileFetch_1.DIRECTORY` — `tFileFetch_1` reads or writes `C://talend_files`, an absolute path: it ties the job to one machine's layout (process/t03_DemoRoutines/demo02_defineProperties_0.1.item:85)
- **absolute_path** `step1CreateTable.tFileInputDelimited_1.FILENAME` — `tFileInputDelimited_1` reads or writes `C:\TalendDemoFiles\talend_files_0.1/in/state.txt`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/Databases/SCD/step1CreateTable_0.1.item:102)
- **absolute_path** `step2ModifyData.tMap_1.TEMPORARY_DATA_DIRECTORY` — `tMap_1` reads or writes `C:/Talend/Logiciels/TOS/TOS2.4.2/Install/TOS-All-r17347-V2.4.2/workspace/temp`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/Databases/SCD/step2ModifyData_0.1.item:206)
- **absolute_path** `tContextLoad_and_file.tFileInputDelimited_1.FILENAME` — `tFileInputDelimited_1` reads or writes `C://talend_files/in/config.txt`, an absolute path: it ties the job to one machine's layout (process/t04_Context/tContextLoad_and_file_0.1.item:84)
- **absolute_path** `tExternalSortRow.tExternalSortRow_1.FILENAME` — `tExternalSortRow_1` reads or writes `C:/talend_file/outputSorted.txt`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/Processing/tExternalSortRow_0.1.item:125)
- **absolute_path** `tExternalSortRow.tExternalSortRow_1.SORT_CMDPATH` — `tExternalSortRow_1` reads or writes `C:/GnuWin32/bin/sort.exe`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/Processing/tExternalSortRow_0.1.item:125)
- **absolute_path** `tFileDelete.tFileDelete_1.DIRECTORY` — `tFileDelete_1` reads or writes `C:/Talend/Logiciels/TOS/TOS2.4.2/Install/TOS-All-r17347-V2.4.2/workspace`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/File/Management/tFileDelete_0.1.item:102)
- **absolute_path** `tFileInputMSDelimited.tFileInputMSDelimited_1.FILENAME` — `tFileInputMSDelimited_1` reads or writes `C:\TalendDemoFiles\talend_files_0.1/in/FileInput/MSInputDelimited.txt`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/File/Input/tFileInputMSDelimited_0.1.item:104)
- **absolute_path** `tFileInputMSXML.tFileInputXML_1.FILENAME` — `tFileInputXML_1` reads or writes `/home/nfrancisco/Talend/talend_files_0.1/in/XML/sample.xml`, an absolute path inside a person's own folders: it exists on one machine (process/t01_Compo/File/Input/tFileInputMSXML_0.1.item:217)
- **absolute_path** `tFileInputMSXML.tFileInputXML_1.TMP_FILENAME` — `tFileInputXML_1` reads or writes `/home/nfrancisco/Talend/Logiciels/TOS/4.0.0M3/Install/TOS-All-r37054-V4.0.0M3/workspace/temp.xml`, an absolute path inside a person's own folders: it exists on one machine (process/t01_Compo/File/Input/tFileInputMSXML_0.1.item:217)
- **absolute_path** `tFileInputXML.tFileInputXML_1.TMP_FILENAME` — `tFileInputXML_1` reads or writes `/home/nfrancisco/Bureau/TOS-All-r39316-V4.0.0RC3/workspace/temp.xml`, an absolute path inside a person's own folders: it exists on one machine (process/t01_Compo/File/Input/tFileInputXML_0.1.item:103)
- **absolute_path** `tMysqlSCD.tMap_1.TEMPORARY_DATA_DIRECTORY` — `tMap_1` reads or writes `C:/Talend/Logiciels/TOS/TOS2.4.2/Install/TOS-All-r17347-V2.4.2/workspace/temp`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/Databases/SCD/tMysqlSCD_0.1.item:144)
- **absolute_path** `tPop.tFileInputMail_1.ATTACHMENT_PATH` — `tFileInputMail_1` reads or writes `D:/TOS-All-r5798-V2.2.0RC1/workspace/`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/Internet/tPop_0.1.item:129)
- **absolute_path** `tSystem.tSystem_1.DIRECTORY` — `tSystem_1` reads or writes `D:/TOS-All-r14257-V2.4.0RC2/workspace`, an absolute path: it ties the job to one machine's layout (process/t01_Compo/System_/tSystem_0.1.item:82)
- **absolute_path** `xmlSample2.tFileInputXML_3.TMP_FILENAME` — `tFileInputXML_3` reads or writes `/home/nfrancisco/Bureau/TOS-All-r39316-V4.0.0RC3/workspace/temp.xml`, an absolute path inside a person's own folders: it exists on one machine (process/t01_Compo/XML/xmlSample2_0.1.item:103)

## Clone clusters

Jobs whose component graphs match after names, tables and columns are removed: one dbt macro or generic model could cover each cluster.

| jobs | least similarity |
|---|---|
| `tLogCatcher1`, `tLogCatcher2` | 1.00 |

## Migration order

Waves over the table graph: a job comes after every job writing a table it reads.

| wave | jobs |
|---|---|
| 1 | `ComponentRow`, `Connection`, `DirectoryGeneration`, `ELTdemo`, `OnSubjobError`, `beforeRunJobs`, `built_inContext_1`, `built_inContext_prompt`, `createTables4ETL`, `demo01_tJava`, `demo02_defineProperties`, `demo03_tMap`, `demo04_tRowGenerator`, `generic_elt_step1`, `jobAndContext`, `jobChildren`, `jobParent`, `jobSample`, `link_if`, `link_with_tDie_tWarn`, `onComponentError`, `priorTest`, `readme`, `repositoryContext`, `scd_step1`, `step1CreateTable`, `step2`, `tAS400Input`, `tAS400Output`, `tAccessOutput`, `tAddCRC`, `tAddLocationFromIP`, `tAdvancedFOXML`, `tAdvancedFileOutputXML`, `tAggregateRow`, `tAggregateSortedRow`, `tAssert`, `tBufferOutput`, `tBufferOutput2`, `tChronometer`, `tContextLoad`, `tContextLoad_and_file`, `tCreateTemporaryFile`, `tDB2Output`, `tDBOutput`, `tDTDValidator`, `tDenormalize`, `tExternalSortRow`, `tExtractDelimitedFields`, `tExtractPositionalFields`, `tExtractRegexFields`, `tFileArchive`, `tFileCompare`, `tFileDelete`, `tFileExist`, `tFileFetch`, `tFileInputARFF`, `tFileInputDelimited`, `tFileInputDelimitedCSV`, `tFileInputExcel`, `tFileInputLDIF`, `tFileInputMSDelimited`, `tFileInputMSPositional`, `tFileInputMSXML`, `tFileInputMail`, `tFileInputPositional`, `tFileInputXML`, `tFileList`, `tFileOutputDelimited`, `tFileOutputDelimitedCSV`, `tFileOutputExcel`, `tFileOutputLDIF`, `tFileOutputMSDelimited`, `tFileOutputMSPositional`, `tFileOutputMSXML`, `tFileOutputPositional`, `tFileOutputXML`, `tFileProperties`, `tFileRegex`, `tFileRowCount`, `tFileTouch`, `tFileUnarchive`, `tFilterColumns`, `tFilterRows`, `tFixedFlowInput`, `tFlowMeter1`, `tFlowMeter2`, `tFlowToIterate`, `tForeach`, `tFuzzyMatch`, `tGroovy`, `tGroovyFile`, `tIntervalMatch`, `tIterateToFlow`, `tJava`, `tJavaFlex`, `tJavaRow`, `tJoin`, `tLibraryLoad`, `tLogCatcher1`, `tLogCatcher2`, `tLoop`, `tMSSqlOutput`, `tMapParse`, `tMap_1`, `tMap_2`, `tMap_3`, `tMap_4`, `tMap_Reload_At_Each_Row_1`, `tMomInput`, `tMomOutput`, `tMsgBox`, `tMsgBox2`, `tMysqlBulkExec`, `tMysqlOutput`, `tMysqlOutputBulk`, `tMysqlOutputBulkExec`, `tMysqlSP`, `tNormalize`, `tOracleOutput`, `tPop`, `tPostgreSQLOutput`, `tPreJob`, `tReplace`, `tReplaceList`, `tReplicate`, `tRowGenerator`, `tRunJobRow`, `tSQLiteOutput`, `tSSH`, `tSVNLogInput`, `tSampleRow`, `tSchemaComplianceCheck`, `tSendMail`, `tServerAlive`, `tSetEnv`, `tSetGlobalVar`, `tSleep`, `tSocketInput`, `tSocketOutput`, `tSortRow`, `tStatCatcher1`, `tStatCatcher2`, `tSybaseOutput`, `tSystem`, `tUniqRow`, `tUnite`, `tWaitForFile`, `tWaitForSocket`, `tWaitForSqlData`, `tWebServiceInput`, `tXSDValidator`, `tXSLT`, `xmlSample2` |
| 2 | `DirectoryService`, `generic_elt_step2`, `scd_step2`, `step2ModifyData`, `tAccessInput`, `tDB2Input`, `tDBInput`, `tMSSqlInput`, `tMap_Reload_At_Each_Row_2`, `tMysqlInput`, `tOracleInput`, `tPostgreSQLInput`, `tSQLiteInput`, `tSybaseInput` |
| 3 | `tMysqlSCD` |

## Per job

Data components lowered to the IR (a stand-in for the translation's fidelity classes, which do not exist yet), expressions by kind, state kept between runs, and what depends on the run.

| job | mapped / data | expressions | state | run time | idioms |
|---|---|---|---|---|---|
| `ComponentRow` | 2 / 4 |  | uses a sequence (Numeric.sequence) |  |  |
| `Connection` | 2 / 2 |  | commits (tMysqlCommit_1); uses a sequence (Numeric.sequence) |  |  |
| `DirectoryGeneration` | 5 / 5 |  |  |  |  |
| `DirectoryService` | 6 / 6 | column 6 |  |  |  |
| `ELTdemo` | 2 / 3 | column 4 |  |  |  |
| `OnSubjobError` | 2 / 2 |  |  |  |  |
| `beforeRunJobs` | 0 / 0 |  |  |  |  |
| `built_inContext_1` | 0 / 0 |  |  |  |  |
| `built_inContext_prompt` | 0 / 0 |  |  |  |  |
| `createTables4ETL` | 3 / 3 |  | uses a sequence (Numeric.sequence) |  |  |
| `demo01_tJava` | 0 / 0 |  |  |  |  |
| `demo02_defineProperties` | 0 / 0 |  |  |  |  |
| `demo03_tMap` | 2 / 2 | column 2, java_method 1 |  |  |  |
| `demo04_tRowGenerator` | 1 / 1 |  |  |  |  |
| `generic_elt_step1` | 4 / 4 |  |  |  |  |
| `generic_elt_step2` | 4 / 4 | arithmetic 2, column 2 | commits (tSQLTemplateCommit_1) |  |  |
| `jobAndContext` | 0 / 0 |  |  |  |  |
| `jobChildren` | 0 / 0 |  |  |  |  |
| `jobParent` | 0 / 0 |  |  |  |  |
| `jobSample` | 2 / 2 |  |  |  |  |
| `link_if` | 1 / 1 |  |  |  |  |
| `link_with_tDie_tWarn` | 1 / 1 |  |  |  |  |
| `onComponentError` | 1 / 1 |  |  |  |  |
| `priorTest` | 0 / 0 |  |  |  |  |
| `readme` | 0 / 0 |  |  |  |  |
| `repositoryContext` | 0 / 0 |  |  |  |  |
| `scd_step1` | 2 / 2 |  | uses a sequence (Numeric.sequence) |  |  |
| `scd_step2` | 12 / 12 | column 14 | UPDATE_OR_INSERT employee; uses a sequence (Numeric.sequence); writes back to employee, which it also reads; writes back to employee_scd, which it also reads |  |  |
| `step1CreateTable` | 2 / 2 |  |  |  |  |
| `step2` | 2 / 2 |  | uses a sequence (Numeric.sequence) |  |  |
| `step2ModifyData` | 8 / 8 | column 1, routine 1 | DELETE states; UPDATE states; writes back to states, which it also reads |  |  |
| `tAS400Input` | 0 / 0 |  |  |  |  |
| `tAS400Output` | 1 / 1 |  |  |  |  |
| `tAccessInput` | 1 / 1 |  |  |  |  |
| `tAccessOutput` | 2 / 2 |  |  |  |  |
| `tAddCRC` | 1 / 1 |  |  |  |  |
| `tAddLocationFromIP` | 2 / 2 |  |  |  |  |
| `tAdvancedFOXML` | 1 / 1 |  |  |  |  |
| `tAdvancedFileOutputXML` | 1 / 1 |  |  |  |  |
| `tAggregateRow` | 2 / 2 |  |  |  |  |
| `tAggregateSortedRow` | 2 / 3 |  |  |  |  |
| `tAssert` | 0 / 0 |  |  |  |  |
| `tBufferOutput` | 2 / 2 |  |  |  |  |
| `tBufferOutput2` | 0 / 0 |  |  |  |  |
| `tChronometer` | 0 / 0 |  |  |  |  |
| `tContextLoad` | 1 / 1 |  |  | 2 context value(s) replaceable |  |
| `tContextLoad_and_file` | 1 / 1 |  |  | 3 context value(s) replaceable |  |
| `tCreateTemporaryFile` | 3 / 3 |  |  |  |  |
| `tDB2Input` | 1 / 1 |  |  |  |  |
| `tDB2Output` | 2 / 2 |  |  |  |  |
| `tDBInput` | 1 / 1 |  |  |  |  |
| `tDBOutput` | 2 / 2 |  |  |  |  |
| `tDTDValidator` | 0 / 0 |  |  |  |  |
| `tDenormalize` | 1 / 2 |  |  |  |  |
| `tExternalSortRow` | 1 / 1 |  |  |  |  |
| `tExtractDelimitedFields` | 1 / 2 |  |  |  |  |
| `tExtractPositionalFields` | 1 / 1 |  |  |  |  |
| `tExtractRegexFields` | 1 / 2 |  |  |  |  |
| `tFileArchive` | 0 / 0 |  |  |  |  |
| `tFileCompare` | 0 / 0 |  |  |  |  |
| `tFileDelete` | 0 / 0 |  | deletes a file (tFileDelete_1) |  |  |
| `tFileExist` | 0 / 0 |  |  |  |  |
| `tFileFetch` | 1 / 1 |  |  |  |  |
| `tFileInputARFF` | 1 / 1 |  |  |  |  |
| `tFileInputDelimited` | 1 / 1 |  |  |  |  |
| `tFileInputDelimitedCSV` | 1 / 1 |  |  |  |  |
| `tFileInputExcel` | 1 / 1 |  |  |  |  |
| `tFileInputLDIF` | 1 / 1 |  |  |  |  |
| `tFileInputMSDelimited` | 1 / 1 |  |  |  |  |
| `tFileInputMSPositional` | 0 / 1 |  |  |  |  |
| `tFileInputMSXML` | 3 / 3 |  |  |  |  |
| `tFileInputMail` | 1 / 1 |  |  |  |  |
| `tFileInputPositional` | 1 / 1 |  |  |  |  |
| `tFileInputXML` | 1 / 1 |  |  |  |  |
| `tFileList` | 0 / 0 |  |  | iterates files |  |
| `tFileOutputDelimited` | 2 / 2 |  |  |  |  |
| `tFileOutputDelimitedCSV` | 2 / 2 |  |  |  |  |
| `tFileOutputExcel` | 2 / 2 |  |  |  |  |
| `tFileOutputLDIF` | 2 / 2 |  |  |  |  |
| `tFileOutputMSDelimited` | 3 / 3 |  | uses a sequence (Numeric.sequence) |  |  |
| `tFileOutputMSPositional` | 2 / 3 |  | uses a sequence (Numeric.sequence) |  |  |
| `tFileOutputMSXML` | 3 / 3 |  |  |  |  |
| `tFileOutputPositional` | 2 / 2 |  |  |  |  |
| `tFileOutputXML` | 2 / 2 |  |  |  |  |
| `tFileProperties` | 0 / 0 |  |  |  |  |
| `tFileRegex` | 1 / 1 |  |  |  |  |
| `tFileRowCount` | 0 / 0 |  |  |  |  |
| `tFileTouch` | 0 / 0 |  |  |  |  |
| `tFileUnarchive` | 0 / 0 |  |  |  |  |
| `tFilterColumns` | 1 / 2 |  |  |  |  |
| `tFilterRows` | 2 / 2 |  |  |  |  |
| `tFixedFlowInput` | 1 / 1 |  |  |  |  |
| `tFlowMeter1` | 2 / 2 |  |  | clock |  |
| `tFlowMeter2` | 2 / 2 |  |  | clock |  |
| `tFlowToIterate` | 2 / 2 |  |  |  |  |
| `tForeach` | 0 / 0 |  |  |  |  |
| `tFuzzyMatch` | 2 / 2 |  |  |  |  |
| `tGroovy` | 0 / 0 |  |  |  |  |
| `tGroovyFile` | 0 / 0 |  |  |  |  |
| `tIntervalMatch` | 2 / 2 |  |  |  |  |
| `tIterateToFlow` | 0 / 0 |  |  | iterates files |  |
| `tJava` | 1 / 1 |  |  |  |  |
| `tJavaFlex` | 1 / 1 |  |  |  |  |
| `tJavaRow` | 1 / 1 |  |  |  |  |
| `tJoin` | 2 / 3 |  |  |  |  |
| `tLibraryLoad` | 0 / 0 |  |  |  |  |
| `tLogCatcher1` | 2 / 2 |  |  |  |  |
| `tLogCatcher2` | 2 / 2 |  |  |  |  |
| `tLoop` | 0 / 0 |  |  |  |  |
| `tMSSqlInput` | 1 / 1 |  |  |  |  |
| `tMSSqlOutput` | 2 / 2 |  |  |  |  |
| `tMapParse` | 3 / 3 | column 2 |  |  |  |
| `tMap_1` | 2 / 2 | arithmetic 1, routine 1 |  |  |  |
| `tMap_2` | 2 / 2 | column 4, literal 1 |  |  |  |
| `tMap_3` | 2 / 2 | column 8, literal 2 |  |  |  |
| `tMap_4` | 3 / 3 | column 9, literal 2 |  |  |  |
| `tMap_Reload_At_Each_Row_1` | 4 / 4 |  | uses a sequence (Numeric.sequence) |  |  |
| `tMap_Reload_At_Each_Row_2` | 3 / 3 | column 2 |  |  |  |
| `tMomInput` | 0 / 0 |  |  |  |  |
| `tMomOutput` | 1 / 1 |  |  |  |  |
| `tMsgBox` | 0 / 0 |  |  |  |  |
| `tMsgBox2` | 0 / 0 |  |  |  |  |
| `tMysqlBulkExec` | 1 / 1 |  |  |  |  |
| `tMysqlInput` | 1 / 1 |  |  |  |  |
| `tMysqlOutput` | 2 / 2 |  |  |  |  |
| `tMysqlOutputBulk` | 1 / 2 |  |  |  |  |
| `tMysqlOutputBulkExec` | 2 / 2 |  |  |  |  |
| `tMysqlSCD` | 4 / 4 | column 2, routine 1 | uses a sequence (Numeric.sequence); writes back to scdstates, which it also reads | clock |  |
| `tMysqlSP` | 2 / 3 |  | uses a sequence (Numeric.sequence) |  |  |
| `tNormalize` | 1 / 2 |  |  |  |  |
| `tOracleInput` | 1 / 1 |  |  |  |  |
| `tOracleOutput` | 2 / 2 |  |  |  |  |
| `tPop` | 1 / 1 |  |  | clock |  |
| `tPostgreSQLInput` | 1 / 1 |  |  |  |  |
| `tPostgreSQLOutput` | 2 / 2 |  |  |  |  |
| `tPreJob` | 1 / 1 |  |  |  |  |
| `tReplace` | 1 / 2 |  |  |  |  |
| `tReplaceList` | 2 / 2 |  |  |  |  |
| `tReplicate` | 1 / 2 |  |  |  |  |
| `tRowGenerator` | 1 / 1 |  |  |  |  |
| `tRunJobRow` | 0 / 0 |  |  |  |  |
| `tSQLiteInput` | 1 / 1 |  |  |  |  |
| `tSQLiteOutput` | 2 / 2 |  |  |  |  |
| `tSSH` | 0 / 0 |  |  |  |  |
| `tSVNLogInput` | 0 / 0 |  |  |  |  |
| `tSampleRow` | 1 / 2 |  | uses a sequence (Numeric.sequence) |  |  |
| `tSchemaComplianceCheck` | 1 / 2 |  |  |  |  |
| `tSendMail` | 0 / 0 |  |  |  |  |
| `tServerAlive` | 0 / 0 |  |  |  |  |
| `tSetEnv` | 0 / 0 |  |  |  |  |
| `tSetGlobalVar` | 0 / 0 |  |  |  |  |
| `tSleep` | 0 / 0 |  |  |  |  |
| `tSocketInput` | 0 / 0 |  |  |  |  |
| `tSocketOutput` | 1 / 1 |  |  |  |  |
| `tSortRow` | 1 / 2 |  |  |  |  |
| `tStatCatcher1` | 3 / 3 |  |  |  |  |
| `tStatCatcher2` | 3 / 3 |  |  |  |  |
| `tSybaseInput` | 1 / 1 |  |  |  |  |
| `tSybaseOutput` | 2 / 2 |  |  |  |  |
| `tSystem` | 0 / 0 |  |  |  |  |
| `tUniqRow` | 2 / 2 |  |  |  |  |
| `tUnite` | 2 / 3 |  |  |  |  |
| `tWaitForFile` | 0 / 0 |  |  |  |  |
| `tWaitForSocket` | 0 / 0 |  |  |  |  |
| `tWaitForSqlData` | 0 / 0 |  |  |  |  |
| `tWebServiceInput` | 0 / 0 |  |  |  |  |
| `tXSDValidator` | 0 / 0 |  |  |  |  |
| `tXSLT` | 0 / 0 |  |  |  |  |
| `xmlSample2` | 3 / 3 | column 7 |  |  |  |

## Target sketch

What a translation would produce, counted before any SQL is written.

|  | count |
|---|---|
| macros for clone clusters | 1 |
| models | 13 |
| seeds or sources from files | 31 |
| snapshots | 0 |

1 older job version(s) are kept beside the latest and were not assessed.

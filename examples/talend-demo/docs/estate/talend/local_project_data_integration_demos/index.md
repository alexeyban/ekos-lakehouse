---
title: "LOCAL_PROJECT_DATA_INTEGRATION_DEMOS"
order: 10
ekos_generated: true
ekos_estate_version: 3
---

# LOCAL_PROJECT_DATA_INTEGRATION_DEMOS

169 job(s), 518 component(s). Data components lowered to the Transformation IR: 225 of 245. Column lineages: 253 (16 with a gap). Findings to review: 178 — see [the assessment](../../talend/local_project_data_integration_demos/assessment.md).

Written by `Talend Open Studio for Big Data-8.0.1.20211109_1610`.

## Orchestration

Jobs run by `tRunJob`, from the jobs no job calls.

- [ELTdemo](../../talend/local_project_data_integration_demos/jobs/eltdemo.md)
  - [createTables4ETL](../../talend/local_project_data_integration_demos/jobs/createtables4etl.md)
- [jobAndContext](../../talend/local_project_data_integration_demos/jobs/jobandcontext.md)
  - [jobChildren](../../talend/local_project_data_integration_demos/jobs/jobchildren.md)
- [jobParent](../../talend/local_project_data_integration_demos/jobs/jobparent.md)
  - [jobChildren](../../talend/local_project_data_integration_demos/jobs/jobchildren.md)
- [tBufferOutput2](../../talend/local_project_data_integration_demos/jobs/tbufferoutput2.md)
  - [tBufferOutput](../../talend/local_project_data_integration_demos/jobs/tbufferoutput.md)
- [tMap_Reload_At_Each_Row_2](../../talend/local_project_data_integration_demos/jobs/tmap_reload_at_each_row_2.md)
  - [tMap_Reload_At_Each_Row_1](../../talend/local_project_data_integration_demos/jobs/tmap_reload_at_each_row_1.md)
- [tMysqlBulkExec](../../talend/local_project_data_integration_demos/jobs/tmysqlbulkexec.md)
  - [tMysqlOutputBulk](../../talend/local_project_data_integration_demos/jobs/tmysqloutputbulk.md)
- [tRunJobRow](../../talend/local_project_data_integration_demos/jobs/trunjobrow.md)
  - [tBufferOutput](../../talend/local_project_data_integration_demos/jobs/tbufferoutput.md)

Not run by any job and running none: [ComponentRow](../../talend/local_project_data_integration_demos/jobs/componentrow.md), [Connection](../../talend/local_project_data_integration_demos/jobs/connection.md), [DirectoryGeneration](../../talend/local_project_data_integration_demos/jobs/directorygeneration.md), [DirectoryService](../../talend/local_project_data_integration_demos/jobs/directoryservice.md), [OnSubjobError](../../talend/local_project_data_integration_demos/jobs/onsubjoberror.md), [beforeRunJobs](../../talend/local_project_data_integration_demos/jobs/beforerunjobs.md), [built_inContext_1](../../talend/local_project_data_integration_demos/jobs/built_incontext_1.md), [built_inContext_prompt](../../talend/local_project_data_integration_demos/jobs/built_incontext_prompt.md), [demo01_tJava](../../talend/local_project_data_integration_demos/jobs/demo01_tjava.md), [demo02_defineProperties](../../talend/local_project_data_integration_demos/jobs/demo02_defineproperties.md), [demo03_tMap](../../talend/local_project_data_integration_demos/jobs/demo03_tmap.md), [demo04_tRowGenerator](../../talend/local_project_data_integration_demos/jobs/demo04_trowgenerator.md), [generic_elt_step1](../../talend/local_project_data_integration_demos/jobs/generic_elt_step1.md), [generic_elt_step2](../../talend/local_project_data_integration_demos/jobs/generic_elt_step2.md), [jobSample](../../talend/local_project_data_integration_demos/jobs/jobsample.md), [link_if](../../talend/local_project_data_integration_demos/jobs/link_if.md), [link_with_tDie_tWarn](../../talend/local_project_data_integration_demos/jobs/link_with_tdie_twarn.md), [onComponentError](../../talend/local_project_data_integration_demos/jobs/oncomponenterror.md), [priorTest](../../talend/local_project_data_integration_demos/jobs/priortest.md), [readme](../../talend/local_project_data_integration_demos/jobs/readme.md), [repositoryContext](../../talend/local_project_data_integration_demos/jobs/repositorycontext.md), [scd_step1](../../talend/local_project_data_integration_demos/jobs/scd_step1.md), [scd_step2](../../talend/local_project_data_integration_demos/jobs/scd_step2.md), [step1CreateTable](../../talend/local_project_data_integration_demos/jobs/step1createtable.md), [step2](../../talend/local_project_data_integration_demos/jobs/step2.md), [step2ModifyData](../../talend/local_project_data_integration_demos/jobs/step2modifydata.md), [tAS400Input](../../talend/local_project_data_integration_demos/jobs/tas400input.md), [tAS400Output](../../talend/local_project_data_integration_demos/jobs/tas400output.md), [tAccessInput](../../talend/local_project_data_integration_demos/jobs/taccessinput.md), [tAccessOutput](../../talend/local_project_data_integration_demos/jobs/taccessoutput.md), [tAddCRC](../../talend/local_project_data_integration_demos/jobs/taddcrc.md), [tAddLocationFromIP](../../talend/local_project_data_integration_demos/jobs/taddlocationfromip.md), [tAdvancedFOXML](../../talend/local_project_data_integration_demos/jobs/tadvancedfoxml.md), [tAdvancedFileOutputXML](../../talend/local_project_data_integration_demos/jobs/tadvancedfileoutputxml.md), [tAggregateRow](../../talend/local_project_data_integration_demos/jobs/taggregaterow.md), [tAggregateSortedRow](../../talend/local_project_data_integration_demos/jobs/taggregatesortedrow.md), [tAssert](../../talend/local_project_data_integration_demos/jobs/tassert.md), [tChronometer](../../talend/local_project_data_integration_demos/jobs/tchronometer.md), [tContextLoad](../../talend/local_project_data_integration_demos/jobs/tcontextload.md), [tContextLoad_and_file](../../talend/local_project_data_integration_demos/jobs/tcontextload_and_file.md), [tCreateTemporaryFile](../../talend/local_project_data_integration_demos/jobs/tcreatetemporaryfile.md), [tDB2Input](../../talend/local_project_data_integration_demos/jobs/tdb2input.md), [tDB2Output](../../talend/local_project_data_integration_demos/jobs/tdb2output.md), [tDBInput](../../talend/local_project_data_integration_demos/jobs/tdbinput.md), [tDBOutput](../../talend/local_project_data_integration_demos/jobs/tdboutput.md), [tDTDValidator](../../talend/local_project_data_integration_demos/jobs/tdtdvalidator.md), [tDenormalize](../../talend/local_project_data_integration_demos/jobs/tdenormalize.md), [tExternalSortRow](../../talend/local_project_data_integration_demos/jobs/texternalsortrow.md), [tExtractDelimitedFields](../../talend/local_project_data_integration_demos/jobs/textractdelimitedfields.md), [tExtractPositionalFields](../../talend/local_project_data_integration_demos/jobs/textractpositionalfields.md), [tExtractRegexFields](../../talend/local_project_data_integration_demos/jobs/textractregexfields.md), [tFileArchive](../../talend/local_project_data_integration_demos/jobs/tfilearchive.md), [tFileCompare](../../talend/local_project_data_integration_demos/jobs/tfilecompare.md), [tFileDelete](../../talend/local_project_data_integration_demos/jobs/tfiledelete.md), [tFileExist](../../talend/local_project_data_integration_demos/jobs/tfileexist.md), [tFileFetch](../../talend/local_project_data_integration_demos/jobs/tfilefetch.md), [tFileInputARFF](../../talend/local_project_data_integration_demos/jobs/tfileinputarff.md), [tFileInputDelimited](../../talend/local_project_data_integration_demos/jobs/tfileinputdelimited.md), [tFileInputDelimitedCSV](../../talend/local_project_data_integration_demos/jobs/tfileinputdelimitedcsv.md), [tFileInputExcel](../../talend/local_project_data_integration_demos/jobs/tfileinputexcel.md), [tFileInputLDIF](../../talend/local_project_data_integration_demos/jobs/tfileinputldif.md), [tFileInputMSDelimited](../../talend/local_project_data_integration_demos/jobs/tfileinputmsdelimited.md), [tFileInputMSPositional](../../talend/local_project_data_integration_demos/jobs/tfileinputmspositional.md), [tFileInputMSXML](../../talend/local_project_data_integration_demos/jobs/tfileinputmsxml.md), [tFileInputMail](../../talend/local_project_data_integration_demos/jobs/tfileinputmail.md), [tFileInputPositional](../../talend/local_project_data_integration_demos/jobs/tfileinputpositional.md), [tFileInputXML](../../talend/local_project_data_integration_demos/jobs/tfileinputxml.md), [tFileList](../../talend/local_project_data_integration_demos/jobs/tfilelist.md), [tFileOutputDelimited](../../talend/local_project_data_integration_demos/jobs/tfileoutputdelimited.md), [tFileOutputDelimitedCSV](../../talend/local_project_data_integration_demos/jobs/tfileoutputdelimitedcsv.md), [tFileOutputExcel](../../talend/local_project_data_integration_demos/jobs/tfileoutputexcel.md), [tFileOutputLDIF](../../talend/local_project_data_integration_demos/jobs/tfileoutputldif.md), [tFileOutputMSDelimited](../../talend/local_project_data_integration_demos/jobs/tfileoutputmsdelimited.md), [tFileOutputMSPositional](../../talend/local_project_data_integration_demos/jobs/tfileoutputmspositional.md), [tFileOutputMSXML](../../talend/local_project_data_integration_demos/jobs/tfileoutputmsxml.md), [tFileOutputPositional](../../talend/local_project_data_integration_demos/jobs/tfileoutputpositional.md), [tFileOutputXML](../../talend/local_project_data_integration_demos/jobs/tfileoutputxml.md), [tFileProperties](../../talend/local_project_data_integration_demos/jobs/tfileproperties.md), [tFileRegex](../../talend/local_project_data_integration_demos/jobs/tfileregex.md), [tFileRowCount](../../talend/local_project_data_integration_demos/jobs/tfilerowcount.md), [tFileTouch](../../talend/local_project_data_integration_demos/jobs/tfiletouch.md), [tFileUnarchive](../../talend/local_project_data_integration_demos/jobs/tfileunarchive.md), [tFilterColumns](../../talend/local_project_data_integration_demos/jobs/tfiltercolumns.md), [tFilterRows](../../talend/local_project_data_integration_demos/jobs/tfilterrows.md), [tFixedFlowInput](../../talend/local_project_data_integration_demos/jobs/tfixedflowinput.md), [tFlowMeter1](../../talend/local_project_data_integration_demos/jobs/tflowmeter1.md), [tFlowMeter2](../../talend/local_project_data_integration_demos/jobs/tflowmeter2.md), [tFlowToIterate](../../talend/local_project_data_integration_demos/jobs/tflowtoiterate.md), [tForeach](../../talend/local_project_data_integration_demos/jobs/tforeach.md), [tFuzzyMatch](../../talend/local_project_data_integration_demos/jobs/tfuzzymatch.md), [tGroovy](../../talend/local_project_data_integration_demos/jobs/tgroovy.md), [tGroovyFile](../../talend/local_project_data_integration_demos/jobs/tgroovyfile.md), [tIntervalMatch](../../talend/local_project_data_integration_demos/jobs/tintervalmatch.md), [tIterateToFlow](../../talend/local_project_data_integration_demos/jobs/titeratetoflow.md), [tJava](../../talend/local_project_data_integration_demos/jobs/tjava.md), [tJavaFlex](../../talend/local_project_data_integration_demos/jobs/tjavaflex.md), [tJavaRow](../../talend/local_project_data_integration_demos/jobs/tjavarow.md), [tJoin](../../talend/local_project_data_integration_demos/jobs/tjoin.md), [tLibraryLoad](../../talend/local_project_data_integration_demos/jobs/tlibraryload.md), [tLogCatcher1](../../talend/local_project_data_integration_demos/jobs/tlogcatcher1.md), [tLogCatcher2](../../talend/local_project_data_integration_demos/jobs/tlogcatcher2.md), [tLoop](../../talend/local_project_data_integration_demos/jobs/tloop.md), [tMSSqlInput](../../talend/local_project_data_integration_demos/jobs/tmssqlinput.md), [tMSSqlOutput](../../talend/local_project_data_integration_demos/jobs/tmssqloutput.md), [tMapParse](../../talend/local_project_data_integration_demos/jobs/tmapparse.md), [tMap_1](../../talend/local_project_data_integration_demos/jobs/tmap_1.md), [tMap_2](../../talend/local_project_data_integration_demos/jobs/tmap_2.md), [tMap_3](../../talend/local_project_data_integration_demos/jobs/tmap_3.md), [tMap_4](../../talend/local_project_data_integration_demos/jobs/tmap_4.md), [tMomInput](../../talend/local_project_data_integration_demos/jobs/tmominput.md), [tMomOutput](../../talend/local_project_data_integration_demos/jobs/tmomoutput.md), [tMsgBox](../../talend/local_project_data_integration_demos/jobs/tmsgbox.md), [tMsgBox2](../../talend/local_project_data_integration_demos/jobs/tmsgbox2.md), [tMysqlInput](../../talend/local_project_data_integration_demos/jobs/tmysqlinput.md), [tMysqlOutput](../../talend/local_project_data_integration_demos/jobs/tmysqloutput.md), [tMysqlOutputBulkExec](../../talend/local_project_data_integration_demos/jobs/tmysqloutputbulkexec.md), [tMysqlSCD](../../talend/local_project_data_integration_demos/jobs/tmysqlscd.md), [tMysqlSP](../../talend/local_project_data_integration_demos/jobs/tmysqlsp.md), [tNormalize](../../talend/local_project_data_integration_demos/jobs/tnormalize.md), [tOracleInput](../../talend/local_project_data_integration_demos/jobs/toracleinput.md), [tOracleOutput](../../talend/local_project_data_integration_demos/jobs/toracleoutput.md), [tPop](../../talend/local_project_data_integration_demos/jobs/tpop.md), [tPostgreSQLInput](../../talend/local_project_data_integration_demos/jobs/tpostgresqlinput.md), [tPostgreSQLOutput](../../talend/local_project_data_integration_demos/jobs/tpostgresqloutput.md), [tPreJob](../../talend/local_project_data_integration_demos/jobs/tprejob.md), [tReplace](../../talend/local_project_data_integration_demos/jobs/treplace.md), [tReplaceList](../../talend/local_project_data_integration_demos/jobs/treplacelist.md), [tReplicate](../../talend/local_project_data_integration_demos/jobs/treplicate.md), [tRowGenerator](../../talend/local_project_data_integration_demos/jobs/trowgenerator.md), [tSQLiteInput](../../talend/local_project_data_integration_demos/jobs/tsqliteinput.md), [tSQLiteOutput](../../talend/local_project_data_integration_demos/jobs/tsqliteoutput.md), [tSSH](../../talend/local_project_data_integration_demos/jobs/tssh.md), [tSVNLogInput](../../talend/local_project_data_integration_demos/jobs/tsvnloginput.md), [tSampleRow](../../talend/local_project_data_integration_demos/jobs/tsamplerow.md), [tSchemaComplianceCheck](../../talend/local_project_data_integration_demos/jobs/tschemacompliancecheck.md), [tSendMail](../../talend/local_project_data_integration_demos/jobs/tsendmail.md), [tServerAlive](../../talend/local_project_data_integration_demos/jobs/tserveralive.md), [tSetEnv](../../talend/local_project_data_integration_demos/jobs/tsetenv.md), [tSetGlobalVar](../../talend/local_project_data_integration_demos/jobs/tsetglobalvar.md), [tSleep](../../talend/local_project_data_integration_demos/jobs/tsleep.md), [tSocketInput](../../talend/local_project_data_integration_demos/jobs/tsocketinput.md), [tSocketOutput](../../talend/local_project_data_integration_demos/jobs/tsocketoutput.md), [tSortRow](../../talend/local_project_data_integration_demos/jobs/tsortrow.md), [tStatCatcher1](../../talend/local_project_data_integration_demos/jobs/tstatcatcher1.md), [tStatCatcher2](../../talend/local_project_data_integration_demos/jobs/tstatcatcher2.md), [tSybaseInput](../../talend/local_project_data_integration_demos/jobs/tsybaseinput.md), [tSybaseOutput](../../talend/local_project_data_integration_demos/jobs/tsybaseoutput.md), [tSystem](../../talend/local_project_data_integration_demos/jobs/tsystem.md), [tUniqRow](../../talend/local_project_data_integration_demos/jobs/tuniqrow.md), [tUnite](../../talend/local_project_data_integration_demos/jobs/tunite.md), [tWaitForFile](../../talend/local_project_data_integration_demos/jobs/twaitforfile.md), [tWaitForSocket](../../talend/local_project_data_integration_demos/jobs/twaitforsocket.md), [tWaitForSqlData](../../talend/local_project_data_integration_demos/jobs/twaitforsqldata.md), [tWebServiceInput](../../talend/local_project_data_integration_demos/jobs/twebserviceinput.md), [tXSDValidator](../../talend/local_project_data_integration_demos/jobs/txsdvalidator.md), [tXSLT](../../talend/local_project_data_integration_demos/jobs/txslt.md), [xmlSample2](../../talend/local_project_data_integration_demos/jobs/xmlsample2.md).

## Jobs

| job | folder | version | tables read | tables written | mapped / data components | findings |
|---|---|---|---|---|---|---|
| [ComponentRow](../../talend/local_project_data_integration_demos/jobs/componentrow.md) | t01_Compo/Databases | 0.1 | 0 | 1 | 2 / 4 | 1 |
| [Connection](../../talend/local_project_data_integration_demos/jobs/connection.md) | t01_Compo/Databases | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [DirectoryGeneration](../../talend/local_project_data_integration_demos/jobs/directorygeneration.md) | t01_Compo/DataQuality/tFuzzyMatch | 0.1 | 1 | 1 | 5 / 5 | 1 |
| [DirectoryService](../../talend/local_project_data_integration_demos/jobs/directoryservice.md) | t01_Compo/DataQuality/tFuzzyMatch | 0.1 | 1 | 0 | 6 / 6 | 2 |
| [ELTdemo](../../talend/local_project_data_integration_demos/jobs/eltdemo.md) | t01_Compo/ELT | 0.1 | 0 | 1 | 2 / 3 | 0 |
| [OnSubjobError](../../talend/local_project_data_integration_demos/jobs/onsubjoberror.md) | t02_Concatening | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [beforeRunJobs](../../talend/local_project_data_integration_demos/jobs/beforerunjobs.md) |  | 0.1 | 0 | 0 | 0 / 0 | 2 |
| [built_inContext_1](../../talend/local_project_data_integration_demos/jobs/built_incontext_1.md) | t04_Context | 0.1 | 0 | 0 | 0 / 0 | 2 |
| [built_inContext_prompt](../../talend/local_project_data_integration_demos/jobs/built_incontext_prompt.md) | t04_Context | 0.1 | 0 | 0 | 0 / 0 | 2 |
| [createTables4ETL](../../talend/local_project_data_integration_demos/jobs/createtables4etl.md) | t01_Compo/ELT | 0.1 | 0 | 1 | 3 / 3 | 0 |
| [demo01_tJava](../../talend/local_project_data_integration_demos/jobs/demo01_tjava.md) | t03_DemoRoutines | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [demo02_defineProperties](../../talend/local_project_data_integration_demos/jobs/demo02_defineproperties.md) | t03_DemoRoutines | 0.1 | 0 | 0 | 0 / 0 | 2 |
| [demo03_tMap](../../talend/local_project_data_integration_demos/jobs/demo03_tmap.md) | t03_DemoRoutines | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [demo04_tRowGenerator](../../talend/local_project_data_integration_demos/jobs/demo04_trowgenerator.md) | t03_DemoRoutines | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [generic_elt_step1](../../talend/local_project_data_integration_demos/jobs/generic_elt_step1.md) | t01_Compo/ELT/Generic | 0.1 | 0 | 2 | 4 / 4 | 1 |
| [generic_elt_step2](../../talend/local_project_data_integration_demos/jobs/generic_elt_step2.md) | t01_Compo/ELT/Generic | 0.1 | 2 | 1 | 4 / 4 | 1 |
| [jobAndContext](../../talend/local_project_data_integration_demos/jobs/jobandcontext.md) | t01_Compo/System_/tRunJob/tRunJobAndContext | 0.1 | 0 | 0 | 0 / 0 | 0 |
| [jobChildren](../../talend/local_project_data_integration_demos/jobs/jobchildren.md) | t01_Compo/System_/tRunJob | 0.1 | 0 | 0 | 0 / 0 | 0 |
| [jobParent](../../talend/local_project_data_integration_demos/jobs/jobparent.md) | t01_Compo/System_/tRunJob | 0.1 | 0 | 0 | 0 / 0 | 0 |
| [jobSample](../../talend/local_project_data_integration_demos/jobs/jobsample.md) | t01_Compo/System_/tRunJobRow | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [link_if](../../talend/local_project_data_integration_demos/jobs/link_if.md) | t02_Concatening | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [link_with_tDie_tWarn](../../talend/local_project_data_integration_demos/jobs/link_with_tdie_twarn.md) | t02_Concatening | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [onComponentError](../../talend/local_project_data_integration_demos/jobs/oncomponenterror.md) | t02_Concatening | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [priorTest](../../talend/local_project_data_integration_demos/jobs/priortest.md) |  | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [readme](../../talend/local_project_data_integration_demos/jobs/readme.md) |  | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [repositoryContext](../../talend/local_project_data_integration_demos/jobs/repositorycontext.md) | t04_Context | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [scd_step1](../../talend/local_project_data_integration_demos/jobs/scd_step1.md) | t01_Compo/Databases/SCD | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [scd_step2](../../talend/local_project_data_integration_demos/jobs/scd_step2.md) | t01_Compo/Databases/SCD | 0.1 | 2 | 2 | 12 / 12 | 1 |
| [step1CreateTable](../../talend/local_project_data_integration_demos/jobs/step1createtable.md) | t01_Compo/Databases/SCD | 0.1 | 0 | 1 | 2 / 2 | 2 |
| [step2](../../talend/local_project_data_integration_demos/jobs/step2.md) | t01_Compo/Orchestration/tWaitForSQL | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [step2ModifyData](../../talend/local_project_data_integration_demos/jobs/step2modifydata.md) | t01_Compo/Databases/SCD | 0.1 | 1 | 1 | 8 / 8 | 2 |
| [tAS400Input](../../talend/local_project_data_integration_demos/jobs/tas400input.md) | t01_Compo/Databases/InOut/AS400 | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tAS400Output](../../talend/local_project_data_integration_demos/jobs/tas400output.md) | t01_Compo/Databases/InOut/AS400 | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tAccessInput](../../talend/local_project_data_integration_demos/jobs/taccessinput.md) | t01_Compo/Databases/InOut/Access | 0.1 | 1 | 0 | 1 / 1 | 1 |
| [tAccessOutput](../../talend/local_project_data_integration_demos/jobs/taccessoutput.md) | t01_Compo/Databases/InOut/Access | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tAddCRC](../../talend/local_project_data_integration_demos/jobs/taddcrc.md) | t01_Compo/DataQuality | 0.2 | 0 | 0 | 1 / 1 | 1 |
| [tAddLocationFromIP](../../talend/local_project_data_integration_demos/jobs/taddlocationfromip.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tAdvancedFOXML](../../talend/local_project_data_integration_demos/jobs/tadvancedfoxml.md) | t01_Compo/XML | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tAdvancedFileOutputXML](../../talend/local_project_data_integration_demos/jobs/tadvancedfileoutputxml.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tAggregateRow](../../talend/local_project_data_integration_demos/jobs/taggregaterow.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tAggregateSortedRow](../../talend/local_project_data_integration_demos/jobs/taggregatesortedrow.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 2 / 3 | 1 |
| [tAssert](../../talend/local_project_data_integration_demos/jobs/tassert.md) | t01_Compo/LogError | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tBufferOutput](../../talend/local_project_data_integration_demos/jobs/tbufferoutput.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 2 / 2 | 0 |
| [tBufferOutput2](../../talend/local_project_data_integration_demos/jobs/tbufferoutput2.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 0 / 0 | 0 |
| [tChronometer](../../talend/local_project_data_integration_demos/jobs/tchronometer.md) | t01_Compo/LogError | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tContextLoad](../../talend/local_project_data_integration_demos/jobs/tcontextload.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tContextLoad_and_file](../../talend/local_project_data_integration_demos/jobs/tcontextload_and_file.md) | t04_Context | 0.1 | 0 | 0 | 1 / 1 | 2 |
| [tCreateTemporaryFile](../../talend/local_project_data_integration_demos/jobs/tcreatetemporaryfile.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 3 / 3 | 1 |
| [tDB2Input](../../talend/local_project_data_integration_demos/jobs/tdb2input.md) | t01_Compo/Databases/InOut/DB2 | 0.1 | 1 | 0 | 1 / 1 | 1 |
| [tDB2Output](../../talend/local_project_data_integration_demos/jobs/tdb2output.md) | t01_Compo/Databases/InOut/DB2 | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tDBInput](../../talend/local_project_data_integration_demos/jobs/tdbinput.md) | t01_Compo/Databases/InOut/DBGeneric | 0.1 | 1 | 0 | 1 / 1 | 1 |
| [tDBOutput](../../talend/local_project_data_integration_demos/jobs/tdboutput.md) | t01_Compo/Databases/InOut/DBGeneric | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tDTDValidator](../../talend/local_project_data_integration_demos/jobs/tdtdvalidator.md) | t01_Compo/XML | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tDenormalize](../../talend/local_project_data_integration_demos/jobs/tdenormalize.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tExternalSortRow](../../talend/local_project_data_integration_demos/jobs/texternalsortrow.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 1 / 1 | 3 |
| [tExtractDelimitedFields](../../talend/local_project_data_integration_demos/jobs/textractdelimitedfields.md) | t01_Compo/Processing/Fields | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tExtractPositionalFields](../../talend/local_project_data_integration_demos/jobs/textractpositionalfields.md) | t01_Compo/Processing/Fields | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tExtractRegexFields](../../talend/local_project_data_integration_demos/jobs/textractregexfields.md) | t01_Compo/Processing/Fields | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tFileArchive](../../talend/local_project_data_integration_demos/jobs/tfilearchive.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tFileCompare](../../talend/local_project_data_integration_demos/jobs/tfilecompare.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tFileDelete](../../talend/local_project_data_integration_demos/jobs/tfiledelete.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 0 / 0 | 2 |
| [tFileExist](../../talend/local_project_data_integration_demos/jobs/tfileexist.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tFileFetch](../../talend/local_project_data_integration_demos/jobs/tfilefetch.md) | t01_Compo/Internet | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFileInputARFF](../../talend/local_project_data_integration_demos/jobs/tfileinputarff.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFileInputDelimited](../../talend/local_project_data_integration_demos/jobs/tfileinputdelimited.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFileInputDelimitedCSV](../../talend/local_project_data_integration_demos/jobs/tfileinputdelimitedcsv.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFileInputExcel](../../talend/local_project_data_integration_demos/jobs/tfileinputexcel.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFileInputLDIF](../../talend/local_project_data_integration_demos/jobs/tfileinputldif.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFileInputMSDelimited](../../talend/local_project_data_integration_demos/jobs/tfileinputmsdelimited.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 2 |
| [tFileInputMSPositional](../../talend/local_project_data_integration_demos/jobs/tfileinputmspositional.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 0 / 1 | 1 |
| [tFileInputMSXML](../../talend/local_project_data_integration_demos/jobs/tfileinputmsxml.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 3 / 3 | 3 |
| [tFileInputMail](../../talend/local_project_data_integration_demos/jobs/tfileinputmail.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFileInputPositional](../../talend/local_project_data_integration_demos/jobs/tfileinputpositional.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFileInputXML](../../talend/local_project_data_integration_demos/jobs/tfileinputxml.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 2 |
| [tFileList](../../talend/local_project_data_integration_demos/jobs/tfilelist.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tFileOutputDelimited](../../talend/local_project_data_integration_demos/jobs/tfileoutputdelimited.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tFileOutputDelimitedCSV](../../talend/local_project_data_integration_demos/jobs/tfileoutputdelimitedcsv.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tFileOutputExcel](../../talend/local_project_data_integration_demos/jobs/tfileoutputexcel.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tFileOutputLDIF](../../talend/local_project_data_integration_demos/jobs/tfileoutputldif.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tFileOutputMSDelimited](../../talend/local_project_data_integration_demos/jobs/tfileoutputmsdelimited.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 3 / 3 | 1 |
| [tFileOutputMSPositional](../../talend/local_project_data_integration_demos/jobs/tfileoutputmspositional.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 2 / 3 | 1 |
| [tFileOutputMSXML](../../talend/local_project_data_integration_demos/jobs/tfileoutputmsxml.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 3 / 3 | 1 |
| [tFileOutputPositional](../../talend/local_project_data_integration_demos/jobs/tfileoutputpositional.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tFileOutputXML](../../talend/local_project_data_integration_demos/jobs/tfileoutputxml.md) | t01_Compo/File/Output | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tFileProperties](../../talend/local_project_data_integration_demos/jobs/tfileproperties.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tFileRegex](../../talend/local_project_data_integration_demos/jobs/tfileregex.md) | t01_Compo/File/Input | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFileRowCount](../../talend/local_project_data_integration_demos/jobs/tfilerowcount.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tFileTouch](../../talend/local_project_data_integration_demos/jobs/tfiletouch.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tFileUnarchive](../../talend/local_project_data_integration_demos/jobs/tfileunarchive.md) | t01_Compo/File/Management | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tFilterColumns](../../talend/local_project_data_integration_demos/jobs/tfiltercolumns.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tFilterRows](../../talend/local_project_data_integration_demos/jobs/tfilterrows.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tFixedFlowInput](../../talend/local_project_data_integration_demos/jobs/tfixedflowinput.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tFlowMeter1](../../talend/local_project_data_integration_demos/jobs/tflowmeter1.md) | t01_Compo/LogError | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tFlowMeter2](../../talend/local_project_data_integration_demos/jobs/tflowmeter2.md) | t01_Compo/LogError | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tFlowToIterate](../../talend/local_project_data_integration_demos/jobs/tflowtoiterate.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tForeach](../../talend/local_project_data_integration_demos/jobs/tforeach.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tFuzzyMatch](../../talend/local_project_data_integration_demos/jobs/tfuzzymatch.md) | t01_Compo/DataQuality | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tGroovy](../../talend/local_project_data_integration_demos/jobs/tgroovy.md) | t01_Compo/CustomCode | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tGroovyFile](../../talend/local_project_data_integration_demos/jobs/tgroovyfile.md) | t01_Compo/CustomCode | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tIntervalMatch](../../talend/local_project_data_integration_demos/jobs/tintervalmatch.md) | t01_Compo/DataQuality | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tIterateToFlow](../../talend/local_project_data_integration_demos/jobs/titeratetoflow.md) | t01_Compo/Orchestration | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tJava](../../talend/local_project_data_integration_demos/jobs/tjava.md) | t01_Compo/CustomCode | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tJavaFlex](../../talend/local_project_data_integration_demos/jobs/tjavaflex.md) | t01_Compo/CustomCode | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tJavaRow](../../talend/local_project_data_integration_demos/jobs/tjavarow.md) | t01_Compo/CustomCode | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tJoin](../../talend/local_project_data_integration_demos/jobs/tjoin.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 2 / 3 | 1 |
| [tLibraryLoad](../../talend/local_project_data_integration_demos/jobs/tlibraryload.md) | t01_Compo/CustomCode | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tLogCatcher1](../../talend/local_project_data_integration_demos/jobs/tlogcatcher1.md) | t01_Compo/LogError | 0.1 | 0 | 0 | 2 / 2 | 2 |
| [tLogCatcher2](../../talend/local_project_data_integration_demos/jobs/tlogcatcher2.md) | t01_Compo/LogError | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tLoop](../../talend/local_project_data_integration_demos/jobs/tloop.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tMSSqlInput](../../talend/local_project_data_integration_demos/jobs/tmssqlinput.md) | t01_Compo/Databases/InOut/MSSQLServer | 0.1 | 1 | 0 | 1 / 1 | 1 |
| [tMSSqlOutput](../../talend/local_project_data_integration_demos/jobs/tmssqloutput.md) | t01_Compo/Databases/InOut/MSSQLServer | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tMapParse](../../talend/local_project_data_integration_demos/jobs/tmapparse.md) | t01_Compo/Processing/tMap | 0.1 | 0 | 0 | 3 / 3 | 1 |
| [tMap_1](../../talend/local_project_data_integration_demos/jobs/tmap_1.md) | t01_Compo/Processing/tMap | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tMap_2](../../talend/local_project_data_integration_demos/jobs/tmap_2.md) | t01_Compo/Processing/tMap | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tMap_3](../../talend/local_project_data_integration_demos/jobs/tmap_3.md) | t01_Compo/Processing/tMap | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tMap_4](../../talend/local_project_data_integration_demos/jobs/tmap_4.md) | t01_Compo/Processing/tMap | 0.1 | 0 | 0 | 3 / 3 | 1 |
| [tMap_Reload_At_Each_Row_1](../../talend/local_project_data_integration_demos/jobs/tmap_reload_at_each_row_1.md) | t01_Compo/Processing/tMap/Reload_At_Each_Row | 0.1 | 0 | 2 | 4 / 4 | 0 |
| [tMap_Reload_At_Each_Row_2](../../talend/local_project_data_integration_demos/jobs/tmap_reload_at_each_row_2.md) | t01_Compo/Processing/tMap/Reload_At_Each_Row | 0.1 | 2 | 0 | 3 / 3 | 0 |
| [tMomInput](../../talend/local_project_data_integration_demos/jobs/tmominput.md) | t01_Compo/Internet | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tMomOutput](../../talend/local_project_data_integration_demos/jobs/tmomoutput.md) | t01_Compo/Internet | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tMsgBox](../../talend/local_project_data_integration_demos/jobs/tmsgbox.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tMsgBox2](../../talend/local_project_data_integration_demos/jobs/tmsgbox2.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tMysqlBulkExec](../../talend/local_project_data_integration_demos/jobs/tmysqlbulkexec.md) | t01_Compo/Databases/Bulk | 0.1 | 0 | 1 | 1 / 1 | 0 |
| [tMysqlInput](../../talend/local_project_data_integration_demos/jobs/tmysqlinput.md) | t01_Compo/Databases/InOut/MySQL | 0.1 | 1 | 0 | 1 / 1 | 1 |
| [tMysqlOutput](../../talend/local_project_data_integration_demos/jobs/tmysqloutput.md) | t01_Compo/Databases/InOut/MySQL | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tMysqlOutputBulk](../../talend/local_project_data_integration_demos/jobs/tmysqloutputbulk.md) | t01_Compo/Databases/Bulk | 0.1 | 0 | 0 | 1 / 2 | 0 |
| [tMysqlOutputBulkExec](../../talend/local_project_data_integration_demos/jobs/tmysqloutputbulkexec.md) | t01_Compo/Databases/Bulk | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tMysqlSCD](../../talend/local_project_data_integration_demos/jobs/tmysqlscd.md) | t01_Compo/Databases/SCD | 0.1 | 2 | 1 | 4 / 4 | 2 |
| [tMysqlSP](../../talend/local_project_data_integration_demos/jobs/tmysqlsp.md) | t01_Compo/Databases/SP | 0.1 | 0 | 1 | 2 / 3 | 1 |
| [tNormalize](../../talend/local_project_data_integration_demos/jobs/tnormalize.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tOracleInput](../../talend/local_project_data_integration_demos/jobs/toracleinput.md) | t01_Compo/Databases/InOut/Oracle | 0.1 | 1 | 0 | 1 / 1 | 1 |
| [tOracleOutput](../../talend/local_project_data_integration_demos/jobs/toracleoutput.md) | t01_Compo/Databases/InOut/Oracle | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tPop](../../talend/local_project_data_integration_demos/jobs/tpop.md) | t01_Compo/Internet | 0.1 | 0 | 0 | 1 / 1 | 2 |
| [tPostgreSQLInput](../../talend/local_project_data_integration_demos/jobs/tpostgresqlinput.md) | t01_Compo/Databases/InOut/PostgreSQL | 0.1 | 1 | 0 | 1 / 1 | 1 |
| [tPostgreSQLOutput](../../talend/local_project_data_integration_demos/jobs/tpostgresqloutput.md) | t01_Compo/Databases/InOut/PostgreSQL | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tPreJob](../../talend/local_project_data_integration_demos/jobs/tprejob.md) | t01_Compo/Orchestration | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tReplace](../../talend/local_project_data_integration_demos/jobs/treplace.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tReplaceList](../../talend/local_project_data_integration_demos/jobs/treplacelist.md) | t01_Compo/DataQuality | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tReplicate](../../talend/local_project_data_integration_demos/jobs/treplicate.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tRowGenerator](../../talend/local_project_data_integration_demos/jobs/trowgenerator.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tRunJobRow](../../talend/local_project_data_integration_demos/jobs/trunjobrow.md) | t01_Compo/System_/tRunJobRow | 0.1 | 0 | 0 | 0 / 0 | 0 |
| [tSQLiteInput](../../talend/local_project_data_integration_demos/jobs/tsqliteinput.md) | t01_Compo/Databases/InOut/SQLite | 0.1 | 1 | 0 | 1 / 1 | 1 |
| [tSQLiteOutput](../../talend/local_project_data_integration_demos/jobs/tsqliteoutput.md) | t01_Compo/Databases/InOut/SQLite | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tSSH](../../talend/local_project_data_integration_demos/jobs/tssh.md) | t01_Compo/System_ | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tSVNLogInput](../../talend/local_project_data_integration_demos/jobs/tsvnloginput.md) | t01_Compo/Internet | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tSampleRow](../../talend/local_project_data_integration_demos/jobs/tsamplerow.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tSchemaComplianceCheck](../../talend/local_project_data_integration_demos/jobs/tschemacompliancecheck.md) | t01_Compo/DataQuality | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tSendMail](../../talend/local_project_data_integration_demos/jobs/tsendmail.md) | t01_Compo/Internet | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tServerAlive](../../talend/local_project_data_integration_demos/jobs/tserveralive.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tSetEnv](../../talend/local_project_data_integration_demos/jobs/tsetenv.md) | t01_Compo/System_ | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tSetGlobalVar](../../talend/local_project_data_integration_demos/jobs/tsetglobalvar.md) | t01_Compo/CustomCode | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tSleep](../../talend/local_project_data_integration_demos/jobs/tsleep.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tSocketInput](../../talend/local_project_data_integration_demos/jobs/tsocketinput.md) | t01_Compo/Internet | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tSocketOutput](../../talend/local_project_data_integration_demos/jobs/tsocketoutput.md) | t01_Compo/Internet | 0.1 | 0 | 0 | 1 / 1 | 1 |
| [tSortRow](../../talend/local_project_data_integration_demos/jobs/tsortrow.md) | t01_Compo/Processing | 0.1 | 0 | 0 | 1 / 2 | 1 |
| [tStatCatcher1](../../talend/local_project_data_integration_demos/jobs/tstatcatcher1.md) | t01_Compo/LogError | 0.1 | 0 | 0 | 3 / 3 | 1 |
| [tStatCatcher2](../../talend/local_project_data_integration_demos/jobs/tstatcatcher2.md) | t01_Compo/LogError | 0.1 | 0 | 0 | 3 / 3 | 1 |
| [tSybaseInput](../../talend/local_project_data_integration_demos/jobs/tsybaseinput.md) | t01_Compo/Databases/InOut/Sybase | 0.1 | 1 | 0 | 1 / 1 | 1 |
| [tSybaseOutput](../../talend/local_project_data_integration_demos/jobs/tsybaseoutput.md) | t01_Compo/Databases/InOut/Sybase | 0.1 | 0 | 1 | 2 / 2 | 1 |
| [tSystem](../../talend/local_project_data_integration_demos/jobs/tsystem.md) | t01_Compo/System_ | 0.1 | 0 | 0 | 0 / 0 | 2 |
| [tUniqRow](../../talend/local_project_data_integration_demos/jobs/tuniqrow.md) | t01_Compo/DataQuality | 0.1 | 0 | 0 | 2 / 2 | 1 |
| [tUnite](../../talend/local_project_data_integration_demos/jobs/tunite.md) | t01_Compo/Orchestration | 0.1 | 0 | 0 | 2 / 3 | 1 |
| [tWaitForFile](../../talend/local_project_data_integration_demos/jobs/twaitforfile.md) | t01_Compo/Misc | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tWaitForSocket](../../talend/local_project_data_integration_demos/jobs/twaitforsocket.md) | t01_Compo/Orchestration | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tWaitForSqlData](../../talend/local_project_data_integration_demos/jobs/twaitforsqldata.md) | t01_Compo/Orchestration/tWaitForSQL | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tWebServiceInput](../../talend/local_project_data_integration_demos/jobs/twebserviceinput.md) | t01_Compo/Internet | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tXSDValidator](../../talend/local_project_data_integration_demos/jobs/txsdvalidator.md) | t01_Compo/XML | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [tXSLT](../../talend/local_project_data_integration_demos/jobs/txslt.md) | t01_Compo/XML | 0.1 | 0 | 0 | 0 / 0 | 1 |
| [xmlSample2](../../talend/local_project_data_integration_demos/jobs/xmlsample2.md) | t01_Compo/XML | 0.1 | 0 | 0 | 3 / 3 | 2 |

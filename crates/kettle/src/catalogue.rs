//! The step catalogue (RFC 0201): every known Kettle step type id, placed in the category the
//! medallion split and the coverage report use.

use serde::{Deserialize, Serialize};

/// Where a step type sits in a translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Reads rows from a database or generates them.
    Input,
    /// Changes each row on its own (rename, retype, calculate, map values).
    RowTransform,
    /// Needs more than one row at a time: sort, group, window, dedupe, (de)normalise.
    SetTransform,
    /// Combines two streams by key or as a product.
    Join,
    /// Looks values up in a table or another stream.
    Lookup,
    /// Writes rows to a table.
    Output,
    /// Routes rows: filter, switch, append, dummy, abort.
    Flow,
    /// User code: JavaScript, Java class, Java expression or filter, formula.
    Code,
    /// Runs another transformation or job.
    SubTransformation,
    /// Reads or writes files (text, Excel, XML, JSON …): outside the lakehouse.
    File,
    /// Run-time plumbing: variables, result rows, logging, delays.
    Orchestration,
    /// A plugin or a type id not in the catalogue.
    Unknown,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Input => "input",
            Category::RowTransform => "row_transform",
            Category::SetTransform => "set_transform",
            Category::Join => "join",
            Category::Lookup => "lookup",
            Category::Output => "output",
            Category::Flow => "flow",
            Category::Code => "code",
            Category::SubTransformation => "sub_transformation",
            Category::File => "file",
            Category::Orchestration => "orchestration",
            Category::Unknown => "unknown",
        }
    }
}

/// `(type id, category, readable name)` for every step type the catalogue knows.
pub const STEPS: &[(&str, Category, &str)] = &[
    // ── input ──
    ("TableInput", Category::Input, "Table input"),
    ("RowGenerator", Category::Input, "Generate rows"),
    ("DataGrid", Category::Input, "Data grid"),
    ("SystemInfo", Category::Input, "Get system info"),
    ("GetVariable", Category::Input, "Get variables"),
    ("RowsFromResult", Category::Input, "Get rows from result"),
    (
        "MappingInput",
        Category::Input,
        "Mapping input specification",
    ),
    ("ExecSQLRow", Category::Input, "Execute row SQL script"),
    // ── row-local ──
    ("SelectValues", Category::RowTransform, "Select values"),
    ("Calculator", Category::RowTransform, "Calculator"),
    ("Constant", Category::RowTransform, "Add constants"),
    (
        "SetValueConstant",
        Category::RowTransform,
        "Set field value to a constant",
    ),
    ("SetValueField", Category::RowTransform, "Set field value"),
    ("ValueMapper", Category::RowTransform, "Value mapper"),
    ("ReplaceString", Category::RowTransform, "Replace in string"),
    (
        "StringOperations",
        Category::RowTransform,
        "String operations",
    ),
    ("StringCut", Category::RowTransform, "Strings cut"),
    ("ConcatFields", Category::RowTransform, "Concat fields"),
    ("FieldSplitter", Category::RowTransform, "Split fields"),
    ("IfNull", Category::RowTransform, "If field value is null"),
    ("NullIf", Category::RowTransform, "Null if"),
    ("NumberRange", Category::RowTransform, "Number range"),
    ("Sequence", Category::RowTransform, "Add sequence"),
    ("CheckSum", Category::RowTransform, "Add a checksum"),
    (
        "FieldsChangeSequence",
        Category::RowTransform,
        "Add value fields changing sequence",
    ),
    ("Validator", Category::RowTransform, "Data validator"),
    (
        "RandomValue",
        Category::RowTransform,
        "Generate random value",
    ),
    ("AddXML", Category::RowTransform, "Add XML"),
    (
        "SplitFieldToRows3",
        Category::SetTransform,
        "Split field to rows",
    ),
    // ── set ──
    ("SortRows", Category::SetTransform, "Sort rows"),
    ("Unique", Category::SetTransform, "Unique rows"),
    (
        "UniqueRowsByHashSet",
        Category::SetTransform,
        "Unique rows (HashSet)",
    ),
    ("GroupBy", Category::SetTransform, "Group by"),
    ("MemoryGroupBy", Category::SetTransform, "Memory group by"),
    ("AnalyticQuery", Category::SetTransform, "Analytic query"),
    ("Denormaliser", Category::SetTransform, "Row denormaliser"),
    ("Normaliser", Category::SetTransform, "Row normaliser"),
    ("SortedMerge", Category::SetTransform, "Sorted merge"),
    ("MergeRows", Category::SetTransform, "Merge rows (diff)"),
    ("SampleRows", Category::SetTransform, "Sample rows"),
    (
        "GroupByStats",
        Category::SetTransform,
        "Univariate statistics",
    ),
    // ── join ──
    ("MergeJoin", Category::Join, "Merge join"),
    ("JoinRows", Category::Join, "Join rows (cartesian product)"),
    ("MultiwayMergeJoin", Category::Join, "Multiway merge join"),
    ("XMLJoin", Category::Join, "XML join"),
    // ── lookup ──
    ("StreamLookup", Category::Lookup, "Stream lookup"),
    ("DBLookup", Category::Lookup, "Database lookup"),
    ("DBJoin", Category::Lookup, "Database join"),
    ("FuzzyMatch", Category::Lookup, "Fuzzy match"),
    ("WebServiceLookup", Category::Lookup, "Web services lookup"),
    ("Rest", Category::Lookup, "REST client"),
    ("HTTP", Category::Lookup, "HTTP client"),
    ("HTTPPOST", Category::Lookup, "HTTP post"),
    // ── output ──
    ("TableOutput", Category::Output, "Table output"),
    ("InsertUpdate", Category::Output, "Insert / update"),
    ("Update", Category::Output, "Update"),
    ("Delete", Category::Output, "Delete"),
    (
        "DimensionLookup",
        Category::Output,
        "Dimension lookup/update",
    ),
    (
        "CombinationLookup",
        Category::Output,
        "Combination lookup/update",
    ),
    (
        "SynchronizeAfterMerge",
        Category::Output,
        "Synchronize after merge",
    ),
    ("ExecSQL", Category::Output, "Execute SQL script"),
    (
        "MappingOutput",
        Category::Output,
        "Mapping output specification",
    ),
    // ── flow ──
    ("FilterRows", Category::Flow, "Filter rows"),
    ("SwitchCase", Category::Flow, "Switch / case"),
    ("Append", Category::Flow, "Append streams"),
    ("Dummy", Category::Flow, "Dummy (do nothing)"),
    ("Abort", Category::Flow, "Abort"),
    ("DetectEmptyStream", Category::Flow, "Detect empty stream"),
    ("BlockingStep", Category::Flow, "Blocking step"),
    (
        "BlockUntilStepsFinish",
        Category::Flow,
        "Block this step until steps finish",
    ),
    ("PrioritizeStreams", Category::Flow, "Prioritize streams"),
    (
        "IdentifyLastRow",
        Category::Flow,
        "Identify last row in a stream",
    ),
    // ── code ──
    (
        "ScriptValueMod",
        Category::Code,
        "Modified JavaScript value",
    ),
    (
        "UserDefinedJavaClass",
        Category::Code,
        "User defined Java class",
    ),
    ("Janino", Category::Code, "User defined Java expression"),
    ("JavaFilter", Category::Code, "Java filter"),
    ("Formula", Category::Code, "Formula"),
    ("RuleExecutor", Category::Code, "Rules executor"),
    ("RuleAccumulator", Category::Code, "Rules accumulator"),
    // ── sub-transformations ──
    (
        "Mapping",
        Category::SubTransformation,
        "Mapping (sub-transformation)",
    ),
    (
        "SimpleMapping",
        Category::SubTransformation,
        "Simple mapping (sub-transformation)",
    ),
    (
        "TransExecutor",
        Category::SubTransformation,
        "Transformation executor",
    ),
    ("JobExecutor", Category::SubTransformation, "Job executor"),
    (
        "SingleThreader",
        Category::SubTransformation,
        "Single threader",
    ),
    (
        "MetaInject",
        Category::SubTransformation,
        "ETL metadata injection",
    ),
    // ── files ──
    ("TextFileInput", Category::File, "Text file input"),
    ("CsvInput", Category::File, "CSV file input"),
    ("ExcelInput", Category::File, "Microsoft Excel input"),
    ("getXMLData", Category::File, "Get data from XML"),
    ("XMLInputStream", Category::File, "XML input stream (StAX)"),
    ("JsonInput", Category::File, "JSON input"),
    ("YamlInput", Category::File, "YAML input"),
    (
        "LoadFileInput",
        Category::File,
        "Load file content in memory",
    ),
    ("GetFileNames", Category::File, "Get file names"),
    ("HL7Input", Category::File, "HL7 input"),
    ("TextFileOutput", Category::File, "Text file output"),
    ("ExcelOutput", Category::File, "Microsoft Excel output"),
    (
        "TypeExitExcelWriterStep",
        Category::File,
        "Microsoft Excel writer",
    ),
    ("XMLOutput", Category::File, "XML output"),
    ("JsonOutput", Category::File, "JSON output"),
    ("AutoDoc", Category::File, "Automatic documentation output"),
    // ── orchestration ──
    ("SetVariable", Category::Orchestration, "Set variables"),
    (
        "RowsToResult",
        Category::Orchestration,
        "Copy rows to result",
    ),
    ("WriteToLog", Category::Orchestration, "Write to log"),
    ("Delay", Category::Orchestration, "Delay row"),
    ("Mail", Category::Orchestration, "Mail"),
    ("ProcessFiles", Category::Orchestration, "Process files"),
];

/// The category of step type `type_id`; [`Category::Unknown`] when the catalogue lacks it.
pub fn category(type_id: &str) -> Category {
    STEPS
        .iter()
        .find(|(id, _, _)| *id == type_id)
        .map(|(_, c, _)| *c)
        .unwrap_or(Category::Unknown)
}

/// The readable name of step type `type_id`, or the id itself.
pub fn readable(type_id: &str) -> &str {
    STEPS
        .iter()
        .find(|(id, _, _)| *id == type_id)
        .map(|(_, _, n)| *n)
        .unwrap_or(type_id)
}

/// What a job entry type does, for coverage and ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryCategory {
    /// START, SUCCESS, ABORT, dummy.
    Control,
    /// Runs a transformation or job.
    Run,
    /// Runs SQL on a connection.
    Sql,
    /// Evaluates or checks a condition (EVAL, TABLE_EXISTS, SIMPLE_EVAL, file exists …).
    Condition,
    /// Sets variables.
    Variables,
    /// Files, FTP, mail, shell, messages: outside the lakehouse.
    External,
    Unknown,
}

/// `(job entry type id, category)` for every entry type the catalogue knows.
pub const ENTRIES: &[(&str, EntryCategory)] = &[
    ("SPECIAL", EntryCategory::Control),
    ("SUCCESS", EntryCategory::Control),
    ("ABORT", EntryCategory::Control),
    ("DUMMY", EntryCategory::Control),
    ("TRANS", EntryCategory::Run),
    ("JOB", EntryCategory::Run),
    ("SQL", EntryCategory::Sql),
    ("EVAL", EntryCategory::Condition),
    ("SIMPLE_EVAL", EntryCategory::Condition),
    ("TABLE_EXISTS", EntryCategory::Condition),
    ("COLUMNS_EXIST", EntryCategory::Condition),
    ("EVAL_TABLE_CONTENT", EntryCategory::Condition),
    ("CHECK_DB_CONNECTIONS", EntryCategory::Condition),
    ("FILE_EXISTS", EntryCategory::Condition),
    ("WAIT_FOR_FILE", EntryCategory::Condition),
    ("SET_VARIABLES", EntryCategory::Variables),
    ("DELAY", EntryCategory::External),
    ("MSGBOX_INFO", EntryCategory::External),
    ("ADD_RESULT_FILENAMES", EntryCategory::External),
    ("CREATE_FILE", EntryCategory::External),
    ("DELETE_FILE", EntryCategory::External),
    ("DELETE_FILES", EntryCategory::External),
    ("COPY_FILES", EntryCategory::External),
    ("MOVE_FILES", EntryCategory::External),
    ("FTP", EntryCategory::External),
    ("SFTP", EntryCategory::External),
    ("MAIL", EntryCategory::External),
    ("SHELL", EntryCategory::External),
    ("WRITE_TO_LOG", EntryCategory::External),
    ("HTTP", EntryCategory::External),
];

/// The category of job entry type `type_id`.
pub fn entry_category(type_id: &str) -> EntryCategory {
    ENTRIES
        .iter()
        .find(|(id, _)| *id == type_id)
        .map(|(_, c)| *c)
        .unwrap_or(EntryCategory::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_type_id_is_listed_once() {
        let ids: BTreeSet<_> = STEPS.iter().map(|(id, _, _)| id).collect();
        assert_eq!(ids.len(), STEPS.len());
        let entries: BTreeSet<_> = ENTRIES.iter().map(|(id, _)| id).collect();
        assert_eq!(entries.len(), ENTRIES.len());
    }

    #[test]
    fn unknown_ids_are_unknown_not_errors() {
        assert_eq!(category("TableInput"), Category::Input);
        assert_eq!(category("com.acme.MyStep"), Category::Unknown);
        assert_eq!(readable("com.acme.MyStep"), "com.acme.MyStep");
        assert_eq!(entry_category("TRANS"), EntryCategory::Run);
    }
}

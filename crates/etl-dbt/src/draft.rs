//! RFC 0205's drafts, the engine-neutral half: what a model is asked to draft (one step, its
//! input and output fields, its original text), parsing its answer, and the checks a drafted
//! expression must pass. Moved from `pentaho-dbt` for Talend (EKOS RFC 0268 phase 3); the prompts
//! and which steps may be drafted stay with each engine.

use ekos_lakehouse_etl_rel::{Draft, Field};
use serde::{Deserialize, Serialize};
use sqlparser::ast::{Expr as SqlExpr, Value, visit_expressions};
use std::collections::BTreeMap;
use std::ops::ControlFlow;

/// One step a model may draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftRequest {
    pub transformation: String,
    pub step: String,
    pub step_type: String,
    pub original: String,
    pub inputs: Vec<Field>,
    /// The fields it must produce; empty for a filter (Java filter) or a plugin.
    pub outputs: Vec<Field>,
    /// Why the translator could not follow it.
    pub reason: String,
}

impl DraftRequest {
    pub fn key(&self) -> (String, String) {
        (self.transformation.clone(), self.step.clone())
    }
}

/// What a model answers.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Answer {
    #[serde(default)]
    pub columns: BTreeMap<String, String>,
    #[serde(default)]
    pub keep_when: Option<String>,
    #[serde(default)]
    pub cannot: Option<String>,
}

/// Parses an answer, tolerating a Markdown fence around the JSON.
pub fn parse_answer(text: &str) -> Result<Answer, String> {
    let t = text.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    let start = t.find('{').ok_or("no JSON object in the answer")?;
    let end = t.rfind('}').ok_or("no JSON object in the answer")?;
    serde_json::from_str(&t[start..=end])
        .map_err(|e| format!("the answer is not the JSON asked for: {e}"))
}

/// Trino functions a drafted expression may call.
const FUNCTIONS: &[&str] = &[
    "upper",
    "lower",
    "trim",
    "ltrim",
    "rtrim",
    "length",
    "substr",
    "substring",
    "replace",
    "concat",
    "coalesce",
    "nullif",
    "greatest",
    "least",
    "abs",
    "round",
    "ceiling",
    "ceil",
    "floor",
    "sqrt",
    "power",
    "mod",
    "sign",
    "ln",
    "log10",
    "exp",
    "year",
    "month",
    "day",
    "hour",
    "minute",
    "second",
    "day_of_week",
    "day_of_year",
    "week_of_year",
    "year_of_week",
    "quarter",
    "date_trunc",
    "date_add",
    "date_diff",
    "current_date",
    "current_timestamp",
    "lpad",
    "rpad",
    "strpos",
    "starts_with",
    "regexp_like",
    "regexp_replace",
    "regexp_extract",
    "format_datetime",
    "parse_datetime",
    "date_format",
    "date_parse",
    "to_hex",
    "md5",
    "sha1",
    "sha256",
    "to_utf8",
    "crc32",
    "levenshtein_distance",
    "split_part",
    "reverse",
    "if",
    "try",
    "try_cast",
    "chr",
    "codepoint",
    "truncate",
    "format",
    "array_join",
    "split",
    "transform",
    "element_at",
    "cardinality",
    "from_unixtime",
    "to_unixtime",
];

/// Numbers a draft may use without the step's code stating them.
const COMMON_NUMBERS: &[&str] = &["0", "1", "100", "1000"];

/// Checks `a` against `r` (RFC 0205 §The checks); the draft, or every finding.
pub fn check(r: &DraftRequest, a: &Answer, model: &str) -> Result<Draft, Vec<String>> {
    let mut findings = Vec::new();
    if let Some(why) = &a.cannot
        && a.columns.is_empty()
    {
        return Err(vec![format!("the model says it cannot: {why}")]);
    }
    for o in &r.outputs {
        if !a.columns.contains_key(&o.name) {
            findings.push(format!("no expression for output `{}`", o.name));
        }
    }
    for k in a.columns.keys() {
        let declared = r.outputs.iter().any(|o| &o.name == k);
        let input = r.inputs.iter().any(|f| &f.name == k);
        if !declared && !(r.outputs.is_empty() && input) {
            findings.push(format!("`{k}` is not an output of the step"));
        }
    }
    if r.outputs.is_empty() && a.columns.is_empty() && a.keep_when.is_none() {
        findings.push("neither columns nor keep_when".into());
    }
    let dialect = sqlparser::dialect::GenericDialect {};
    let mut check_one = |what: &str, sql: &str| {
        let parsed = sqlparser::parser::Parser::new(&dialect)
            .try_with_sql(sql)
            .and_then(|mut p| p.parse_expr());
        let e = match parsed {
            Ok(e) => e,
            Err(err) => {
                findings.push(format!("{what}: does not parse: {err}"));
                return;
            }
        };
        let _ = visit_expressions(&e, |x| {
            match x {
                SqlExpr::Identifier(i) => {
                    if !r
                        .inputs
                        .iter()
                        .any(|f| f.name.eq_ignore_ascii_case(&i.value))
                    {
                        findings.push(format!("{what}: `{}` is not an input field", i.value));
                    }
                }
                SqlExpr::CompoundIdentifier(parts) => {
                    findings.push(format!(
                        "{what}: `{}` names something other than an input field",
                        parts
                            .iter()
                            .map(|p| p.value.as_str())
                            .collect::<Vec<_>>()
                            .join(".")
                    ));
                }
                SqlExpr::Subquery(_) | SqlExpr::Exists { .. } | SqlExpr::InSubquery { .. } => {
                    findings.push(format!("{what}: a sub-query"));
                }
                SqlExpr::Function(f) => {
                    let name = f.name.to_string().to_ascii_lowercase();
                    if !FUNCTIONS.contains(&name.as_str()) {
                        findings.push(format!("{what}: function `{name}` is not on the list"));
                    }
                }
                SqlExpr::Value(Value::SingleQuotedString(s))
                    if !s.is_empty() && !r.original.contains(s.as_str()) =>
                {
                    findings.push(format!(
                        "{what}: the literal '{s}' is not in the step's code"
                    ));
                }
                SqlExpr::Value(Value::Number(n, _))
                    if !COMMON_NUMBERS.contains(&n.as_str())
                        && !r.original.contains(n.as_str()) =>
                {
                    findings.push(format!("{what}: the number {n} is not in the step's code"));
                }
                _ => {}
            }
            ControlFlow::<()>::Continue(())
        });
    };
    for (k, v) in &a.columns {
        check_one(&format!("`{k}`"), v);
    }
    if let Some(p) = &a.keep_when {
        check_one("keep_when", p);
    }
    if !findings.is_empty() {
        findings.sort();
        findings.dedup();
        return Err(findings);
    }
    Ok(Draft {
        columns: a
            .columns
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        keep_when: a.keep_when.clone(),
        model: model.to_string(),
    })
}

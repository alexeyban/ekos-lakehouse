//! RFC 0205: which gap steps a model may draft, what it is asked, and the checks its answer must
//! pass. Pure: the call itself is the command's (through the RFC 0197 router).

use crate::Settings;
use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_kettle::{Category, Package};
use ekos_lakehouse_kettle_expr::KType;
use ekos_lakehouse_kettle_rel::{Draft, Field, Op};
use serde::{Deserialize, Serialize};
use sqlparser::ast::{Expr as SqlExpr, Value, visit_expressions};
use std::collections::BTreeMap;
use std::ops::ControlFlow;

/// Bump when the prompt or the answer format changes (it is part of the router's cache key).
pub const PROMPT_VERSION: &str = "pentaho-draft-v1";

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

/// The steps of `package` a model may draft (RFC 0205 §Which steps).
pub fn requests(package: &Package, plan: &Plan, s: &Settings) -> Vec<DraftRequest> {
    let mut out = Vec::new();
    for (t, ir) in crate::build_irs(package, plan, s) {
        for n in &ir.nodes {
            let Op::Gap { reason } = &n.op else { continue };
            if n.step.contains('/') || n.inputs.len() != 1 {
                continue;
            }
            let Some(step) = t.step(&n.step) else {
                continue;
            };
            let code = step.category == Category::Code;
            let plugin =
                step.category == Category::Unknown && s.plugin_code.contains_key(&step.type_id);
            if !code && !plugin {
                continue;
            }
            let input = ir.node(n.inputs[0]);
            if input.is_gap() || input.schema.iter().any(|f| f.ty == KType::Unknown) {
                continue;
            }
            let outputs = ekos_lakehouse_kettle_rel::declared_outputs(step);
            if outputs.is_empty() && step.type_id != "JavaFilter" && !plugin {
                continue;
            }
            let original = if plugin {
                s.plugin_code[&step.type_id].clone()
            } else {
                crate::original_text(step)
            };
            out.push(DraftRequest {
                transformation: t.path.clone(),
                step: n.step.clone(),
                step_type: n.step_type.clone(),
                original,
                inputs: input.schema.clone(),
                outputs,
                reason: reason.clone(),
            });
        }
    }
    out
}

const SYSTEM: &str = "You translate one step of a Pentaho Data Integration (Kettle) transformation into \
Trino SQL. The step runs once per input row. Answer with one JSON object and nothing else:\n\
{\"columns\": {\"<output field>\": \"<Trino SQL expression>\"}, \"keep_when\": \"<Trino SQL predicate, or null>\", \"cannot\": \"<reason, or null>\"}\n\
Rules:\n\
- Read only the input fields listed, written in double quotes (\"field\"). No other tables, no sub-queries.\n\
- Give an expression for every output field listed, and no others.\n\
- keep_when is the condition under which the row is kept; null when every row is kept.\n\
- Use only string and number literals that appear in the step's code.\n\
- Kettle reads an empty string as null; a null concatenated in Java or JavaScript prints \"null\".\n\
- JavaScript divides in floating point; Java divides integers by truncation.\n\
- If the step cannot be expressed per row (it keeps state across rows, loops over rows, reads files or \
calls services), set \"cannot\" to the reason and leave columns empty.";

/// The system and user messages for `r`; `feedback` holds the previous attempt's findings.
pub fn prompt(r: &DraftRequest, feedback: &[String]) -> (String, String) {
    let fields = |fs: &[Field]| {
        fs.iter()
            .map(|f| format!("- \"{}\" ({:?})", f.name, f.ty))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut user = format!(
        "Step `{}` of type {} in {}.\nThe translator could not follow it: {}\n\nInput fields:\n{}\n\n",
        r.step,
        r.step_type,
        r.transformation,
        r.reason,
        fields(&r.inputs)
    );
    if r.outputs.is_empty() {
        user.push_str(
            "Output fields: none of its own (it filters rows, or changes input fields).\n\n",
        );
    } else {
        user.push_str(&format!("Output fields:\n{}\n\n", fields(&r.outputs)));
    }
    user.push_str(&format!("The step's code:\n```\n{}\n```\n", r.original));
    if !feedback.is_empty() {
        user.push_str(&format!(
            "\nYour previous answer was refused:\n{}\nAnswer again with the JSON object only.\n",
            feedback
                .iter()
                .map(|f| format!("- {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    (SYSTEM.to_string(), user)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> DraftRequest {
        DraftRequest {
            transformation: "t.ktr".into(),
            step: "js".into(),
            step_type: "ScriptValueMod".into(),
            original: "var tier = amount > 500 ? 'big' : 'small';\nfor (i = 0; i < 3; i++) {}"
                .into(),
            inputs: vec![
                Field::new("amount", KType::Number),
                Field::new("status", KType::String),
            ],
            outputs: vec![Field::new("tier", KType::String)],
            reason: "`for` in the step's code".into(),
        }
    }

    #[test]
    fn a_checked_answer_becomes_a_draft() {
        let a = parse_answer("```json\n{\"columns\": {\"tier\": \"CASE WHEN \\\"amount\\\" > 500 THEN 'big' ELSE 'small' END\"}, \"keep_when\": null, \"cannot\": null}\n```").unwrap();
        let d = check(&req(), &a, "m").unwrap();
        assert_eq!(d.columns[0].0, "tier");
        assert!(prompt(&req(), &[]).1.contains("\"amount\" (Number)"));
    }

    #[test]
    fn invented_atoms_and_missing_outputs_are_refused() {
        let bad = Answer {
            columns: [(
                "tier".to_string(),
                "CASE WHEN \"price\" > 900 THEN 'huge' ELSE soundex(\"status\") END".to_string(),
            )]
            .into(),
            keep_when: Some("x.y = 1".into()),
            cannot: None,
        };
        let f = check(&req(), &bad, "m").unwrap_err();
        let all = f.join(" | ");
        for want in [
            "`price` is not an input field",
            "900",
            "'huge'",
            "soundex",
            "x.y",
        ] {
            assert!(all.contains(want), "{want}: {all}");
        }
        let missing = Answer::default();
        assert!(
            check(&req(), &missing, "m")
                .unwrap_err()
                .iter()
                .any(|x| x.contains("no expression for output `tier`"))
        );
        let cannot = Answer {
            cannot: Some("loops over rows".into()),
            ..Answer::default()
        };
        assert!(check(&req(), &cannot, "m").unwrap_err()[0].contains("cannot"));
        assert!(parse_answer("sorry").is_err());
    }
}

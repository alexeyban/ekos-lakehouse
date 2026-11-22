//! RFC 0205: which gap steps a model may draft, what it is asked, and the checks its answer must
//! pass. Pure: the call itself is the command's (through the RFC 0197 router).

use crate::Settings;
// The checks every engine's drafts pass, moved to `etl-dbt` (EKOS RFC 0268 phase 3); re-exported.
use ekos_lakehouse_bronze::plan::Plan;
pub use ekos_lakehouse_etl_dbt::draft::{Answer, DraftRequest, check, parse_answer};
use ekos_lakehouse_kettle::{Category, Package};
use ekos_lakehouse_kettle_expr::KType;
use ekos_lakehouse_kettle_rel::{Field, Op};

/// Bump when the prompt or the answer format changes (it is part of the router's cache key).
pub const PROMPT_VERSION: &str = "pentaho-draft-v1";

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

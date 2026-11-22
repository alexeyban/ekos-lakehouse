//! RFC 0205's drafts for Talend (EKOS RFC 0268 phase 3): which gaps a model may draft, and what
//! it is asked. The checks its answer must pass are the engine-neutral ones of `etl-dbt`. Pure:
//! the call itself is the command's, through the RFC 0197 router (an Ollama route needs no
//! permission; any other needs `--allow-metered`).

use ekos_lakehouse_etl_dbt::draft::DraftRequest;
use ekos_lakehouse_etl_rel::NoCatalog;
use ekos_lakehouse_talend_rel::{Options, lower_job_with};
use ekos_talend::Project;

pub use ekos_lakehouse_etl_dbt::draft::{Answer, check, parse_answer};

/// Bump when the prompt or the answer format changes (it is part of the router's cache key).
pub const PROMPT_VERSION: &str = "talend-draft-v2";

/// The step name a request carries: the component, and the output port when it has one.
pub fn step_of(component: &str, port: &str) -> String {
    if port.is_empty() {
        component.to_string()
    } else {
        format!("{component}:{port}")
    }
}

/// The `(job, component, port)` a request's `(transformation, step)` stands for.
pub fn key_of(transformation: &str, step: &str) -> (String, String, String) {
    let (c, p) = step.split_once(':').unwrap_or((step, ""));
    (transformation.to_string(), c.to_string(), p.to_string())
}

/// The gaps of the project's latest jobs a model may draft: a component's output from one input
/// whose fields are all known, with its code or its tMap output's expressions.
pub fn requests(project: &Project, options: &Options) -> Vec<DraftRequest> {
    let mut out = Vec::new();
    for j in project.jobs.iter().filter(|j| j.latest) {
        let job = project.expand(j).job;
        let l = lower_job_with(&job, &project.routines, &NoCatalog, options);
        for d in l.draftable {
            out.push(DraftRequest {
                transformation: d.job,
                step: step_of(&d.component, &d.port),
                step_type: d.component_type,
                original: d.original,
                inputs: d.inputs,
                outputs: d.outputs,
                reason: d.reason,
            });
        }
    }
    out
}

const SYSTEM: &str = "You translate one output of one Talend Open Studio component into Trino SQL. The \
component runs once per input row. Answer with one JSON object and nothing else:\n\
{\"columns\": {\"<output field>\": \"<Trino SQL expression>\"}, \"keep_when\": \"<Trino SQL predicate, or null>\", \"cannot\": \"<reason, or null>\"}\n\
Rules:\n\
- Read only the input fields listed, by their SQL names in double quotes (\"field\"), never by their Java \
names (`row1.COL`, `input_row.COL`): each field shows its Java name. No other tables, no sub-queries.\n\
- Give an expression for every output field listed, and no others.\n\
- keep_when is the output's filter (the row is kept when it is true); null when every row is kept.\n\
- Use only string and number literals that appear in the component's text.\n\
- The text is Java: `a.equals(b)` is equality, `==` on strings compares references, `+` on a string \
concatenates and prints null as \"null\", integer division truncates. Talend's routines: \
StringHandling, TalendDate (Java date patterns), Numeric, Relational.ISNULL.\n\
- If the output cannot be expressed per row (it keeps state across rows, reads files, calls services, \
or sets context or globalMap values), set \"cannot\" to the reason and leave columns empty.";

/// The system and user messages for `r`; `feedback` holds the previous attempt's findings.
pub fn prompt(r: &DraftRequest, feedback: &[String]) -> (String, String) {
    let fields = |fs: &[ekos_lakehouse_etl_rel::Field]| {
        fs.iter()
            .map(|f| format!("- \"{}\" ({:?})", f.name, f.ty))
            .collect::<Vec<_>>()
            .join("\n")
    };
    // Each input field with the Java the component's text reads it by: a tMap's `row1.COL` is the
    // field `row1__COL`; a code component's `input_row.COL` (or `<row>.COL`) is `COL`.
    let inputs: String = r
        .inputs
        .iter()
        .map(|f| match f.name.split_once("__") {
            Some((row, col)) if !row.is_empty() => {
                format!("- \"{}\" ({:?}) — in Java `{row}.{col}`", f.name, f.ty)
            }
            _ => format!(
                "- \"{}\" ({:?}) — in Java `input_row.{}`",
                f.name, f.ty, f.name
            ),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut user = format!(
        "Component `{}` ({}) of job {}.\nThe translator could not follow it: {}\n\nInput fields:\n{}\n\nOutput fields:\n{}\n\nThe component's text:\n```\n{}\n```\n",
        r.step,
        r.step_type,
        r.transformation,
        r.reason,
        inputs,
        fields(&r.outputs),
        r.original
    );
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

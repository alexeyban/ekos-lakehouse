//! EKOS RFC 0257 §3.6 — `lakehouse marts suggest`: free text on a step → one proposed edit.
//!
//! The prompt, the schema and the grounding are `ekos_lakehouse_gold::assist` (pure). Here: the
//! step and its context from the project, the model call — a local Ollama only (the URL must be
//! a loopback address; nothing metered is ever called) — and the answer as the person sees it,
//! with the `marts edit` command that previews it. Nothing is recorded: the person runs the edit,
//! which goes through every gate a hand-made edit does.

use ekos_lakehouse_bronze::plan::Plan;
use ekos_lakehouse_gold::assist::{self, Answer, Judged};
use ekos_lakehouse_gold::{MartSpec, edits::StepEdit};
use ekos_recovery::{LlmProvider, LlmRequest, OllamaProvider};
use serde_json::{Value, json};

/// Only a model on this machine: a URL whose host is a loopback address.
pub fn local_only(url: &str) -> anyhow::Result<()> {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', ':'])
        .next()
        .unwrap_or_default();
    anyhow::ensure!(
        matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1"),
        "the assistant runs on a local Ollama only, not {url}"
    );
    Ok(())
}

/// Everything one request is answered from.
pub struct Ask<'a> {
    pub spec: &'a MartSpec,
    /// `<type>: <summary>` of the step.
    pub step: String,
    pub step_type: String,
    pub tables: Vec<String>,
    pub plan: &'a Plan,
    pub notes: Vec<String>,
    pub request: &'a str,
}

/// The model's answer, judged; an edit on the wrong kind of step is shown with the reason.
pub async fn suggest(a: &Ask<'_>, model: &str, url: &str) -> anyhow::Result<(Judged, String)> {
    local_only(url)?;
    let user = assist::prompt(a.spec, &a.step, &a.tables, a.plan, &a.notes, a.request);
    // The tables' columns make a long prompt: a window that holds it (RFC 0145).
    let provider = OllamaProvider::new(model, url).with_context_window(Some(16384));
    let resp = provider
        .complete(&LlmRequest {
            system: assist::SYSTEM,
            user: &user,
            prompt_version: assist::PROMPT_VERSION,
            max_tokens: 300,
            history: &[],
        })
        .await
        .map_err(|e| anyhow::anyhow!("{model} at {url}: {e}"))?;
    let mut j = assist::judge(&resp.content, a.spec, &a.tables, a.plan, a.request);
    if let Some(Answer::Edit(e)) = &j.answer
        && let Err(why) = crate::mart_edit::on_step(e, &a.step_type)
    {
        // `marts edit` would refuse it: never shown as an edit (found on llama3, which answered
        // "stop splitting by customer" with a filter removal on the aggregate step).
        j.note = Some(format!("changed: {why}"));
        j.answer = Some(Answer::Question(format!(
            "{why}. Is that what you mean? If so, select that step and ask again"
        )));
    }
    Ok((j, resp.content))
}

/// What the person sees.
pub fn view(mart: &str, step: u32, j: &Judged) -> Value {
    let command = match &j.answer {
        Some(Answer::Edit(e)) => Some(format!(
            "lakehouse marts edit --mart {mart} --step {step} --edit '{}' --preview",
            serde_json::to_string(e).unwrap_or_default()
        )),
        _ => None,
    };
    let (kind, text) = match &j.answer {
        Some(Answer::Edit(e)) => ("edit", describe(e)),
        Some(Answer::Question(q)) => ("question", q.clone()),
        Some(Answer::NotExpressible(w)) => (
            "not_expressible",
            format!("{w} — write it as a comment on the step instead"),
        ),
        None => ("dropped", "the assistant's answer was not usable".into()),
    };
    json!({
        "kind": kind,
        "text": text,
        "answer": j.answer,
        "note": j.note,
        "invented": j.invented,
        "preview": command,
    })
}

fn describe(e: &StepEdit) -> String {
    match e {
        StepEdit::AddFilter { filter } => format!("add the filter `{filter}`"),
        StepEdit::ChangeFilter { from, to } => format!("change the filter `{from}` to `{to}`"),
        StepEdit::DropFilter { filter } => format!("remove the filter `{filter}`"),
        StepEdit::ChangeJoin { term, via } => format!("join `{term}`'s table via `{via}`"),
        StepEdit::AddDimension { name, definition } => {
            format!("add the dimension `{name}`: {definition}")
        }
        StepEdit::DropDimension { name } => format!("remove the dimension `{name}`"),
        StepEdit::AddMeasure { name, definition } => {
            format!("add the measure `{name}`: {definition}")
        }
        StepEdit::DropMeasure { name } => format!("remove the measure `{name}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_local_model_is_called() {
        assert!(local_only("http://127.0.0.1:11434").is_ok());
        assert!(local_only("http://localhost:11434/").is_ok());
        assert!(local_only("https://api.example.com/v1").is_err());
        assert!(local_only("http://10.0.0.5:11434").is_err());
    }
}

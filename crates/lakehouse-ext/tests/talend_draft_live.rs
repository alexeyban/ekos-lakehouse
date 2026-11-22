//! EKOS RFC 0268 phase 3: a local model drafts the gaps of a Talend fixture, through the real
//! router and the real checks, and the drafts are translated. Never a metered model: the route is
//! Ollama's, on this machine.
//!
//! Needs `EKOS_OLLAMA_MODEL` (e.g. `qwen2.5-coder:3b`) and a fixture (`EKOS_TALEND_FIXTURE_B` …
//! `_E`); skipped, saying so, otherwise.

use std::path::{Path, PathBuf};

use ekos_lakehouse_talend_dbt::{Settings, draft, translate};
use ekos_talend::{PROJECT_MARKER, Project, ProjectFile, Role, read_project, role};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !ekos_talend::SKIPPED_DIRS.contains(&name.as_str()) {
                walk(&p, out);
            }
        } else {
            out.push(p);
        }
    }
}

fn projects(root: &Path) -> Vec<Project> {
    let mut files = Vec::new();
    walk(root, &mut files);
    files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == PROJECT_MARKER))
        .map(|marker| {
            let dir = marker.parent().unwrap();
            let mut inside = Vec::new();
            walk(dir, &mut inside);
            let pf: Vec<ProjectFile> = inside
                .iter()
                .filter_map(|p| {
                    let rel = p
                        .strip_prefix(dir)
                        .ok()?
                        .to_string_lossy()
                        .replace('\\', "/");
                    (role(&rel) != Role::Other).then_some(())?;
                    Some(ProjectFile {
                        text: std::fs::read_to_string(p).ok()?,
                        path: rel,
                    })
                })
                .collect();
            read_project(&dir.file_name().unwrap().to_string_lossy(), &pf)
        })
        .collect()
}

#[test]
fn a_local_model_drafts_talend_gaps_through_the_checks() {
    let Ok(model) = std::env::var("EKOS_OLLAMA_MODEL") else {
        eprintln!("skipped: EKOS_OLLAMA_MODEL is not set");
        return;
    };
    let llm: ekos_compiler_core::config::LlmConfig = serde_json::from_value(serde_json::json!({
        "provider": "ollama",
        "model": model,
    }))
    .expect("an Ollama route");
    let state = tempfile::tempdir().unwrap();
    let router = ekos_recovery::LlmRouter::new(&llm, state.path());
    let route = router.route(ekos_recovery::Task::Translate);
    assert_eq!(route.provider, "ollama", "never a metered model here");
    let provider = router.provider(ekos_recovery::Task::Translate).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (mut asked, mut drafted, mut enabled_gain) = (0usize, 0usize, 0isize);
    for var in [
        "EKOS_TALEND_FIXTURE_B",
        "EKOS_TALEND_FIXTURE_C",
        "EKOS_TALEND_FIXTURE_D",
    ] {
        let Some(root) = std::env::var_os(var).map(PathBuf::from) else {
            continue;
        };
        for p in projects(&root) {
            if p.jobs.is_empty() {
                continue;
            }
            let plan = ekos_lakehouse_bronze::plan::plan(&[], &Default::default());
            let mut s = Settings {
                project: p.root.clone(),
                locale: ekos_lakehouse_talend_dbt::Locale::parse("fr_FR"),
                ..Settings::default()
            };
            let options = ekos_lakehouse_talend_rel::Options {
                locale: s.locale,
                drafts: Default::default(),
            };
            let requests = draft::requests(&p, &options);
            if requests.is_empty() {
                continue;
            }
            let before = translate(&p, &plan, &s);
            let (drafts, outcomes) = rt.block_on(ekos_lakehouse_ext::pentaho_cmd::draft_all_with(
                &requests,
                provider.as_ref(),
                3,
                &draft::prompt,
                draft::PROMPT_VERSION,
            ));
            for o in &outcomes {
                eprintln!(
                    "{var} {}:{} drafted={} attempts={} {}",
                    o.transformation,
                    o.step,
                    o.drafted,
                    o.attempts,
                    o.findings.first().cloned().unwrap_or_default()
                );
            }
            asked += requests.len();
            drafted += drafts.len();
            s.drafts = drafts
                .into_iter()
                .map(|((job, step), d)| (draft::key_of(&job, &step), d))
                .collect();
            let after = translate(&p, &plan, &s);
            let enabled = |t: &ekos_lakehouse_talend_dbt::Translation| {
                t.report.models.iter().filter(|m| m.enabled).count() as isize
            };
            enabled_gain += enabled(&after) - enabled(&before);
            // A drafted model is labelled so, and no gate disables one.
            for m in &after.report.models {
                for g in &m.gaps {
                    assert!(
                        ![
                            "does not parse as Trino",
                            "anti-invention",
                            "Java text left",
                            "(scope)"
                        ]
                        .iter()
                        .any(|x| g.contains(x)),
                        "{}: a gate disabled {}: {g}",
                        p.root,
                        m.name
                    );
                }
            }
            assert!(
                s.drafts.is_empty()
                    || after
                        .report
                        .components
                        .iter()
                        .any(|c| c.fidelity == ekos_lakehouse_etl_expr::Fidelity::LlmDrafted)
            );
        }
    }
    eprintln!(
        "{asked} gaps asked, {drafted} drafted by {model}, {enabled_gain} models more enabled"
    );
}

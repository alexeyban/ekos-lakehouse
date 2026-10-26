//! EKOS RFC 0230: step 2 — the source-estate documentation (EKOS RFC 0214), generated and
//! published to the documentation store the project names.
//!
//! The publishers are lakehouse-docs' (RFC 0208), compiled into the binary and written to the
//! state directory before each run, so an installed binary carries the version it was built with.
//! Credentials stay in the environment: each publisher reads its own variables.

use ekos::extension::CommandContext;
use ekos_semantic::workflow::{Step, StepState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The publishers, as lakehouse-docs ships them.
pub const PUBLISHERS: &[(&str, &str)] = &[
    (
        "wiki_common.py",
        include_str!("../../../tools/lakehouse-docs/publish/wiki_common.py"),
    ),
    (
        "publish_github_wiki.py",
        include_str!("../../../tools/lakehouse-docs/publish/publish_github_wiki.py"),
    ),
    (
        "publish_azure_devops_wiki.py",
        include_str!("../../../tools/lakehouse-docs/publish/publish_azure_devops_wiki.py"),
    ),
    (
        "publish_confluence.py",
        include_str!("../../../tools/lakehouse-docs/publish/publish_confluence.py"),
    ),
];

/// Where `ekos docs generate --layout estate` records its latest run (EKOS RFC 0218).
pub const ESTATE_RECORD: &str = "docs/estate.json";

/// `[extensions.lakehouse.docs]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct DocsSettings {
    /// Relative to the project directory. Default `docs/estate`.
    pub output: Option<String>,
    pub target: Option<Target>,
    /// The interpreter that runs the publisher. Default `python3`.
    pub python: Option<String>,
    // github
    pub repo: Option<String>,
    pub host: Option<String>,
    pub prefix: Option<String>,
    /// `replace` (the publisher's default: the wiki's `_Sidebar.md` becomes this tree) or `keep`
    /// (leave a sidebar the wiki already has).
    pub sidebar: Option<String>,
    // azure-devops
    pub org_url: Option<String>,
    pub project: Option<String>,
    pub wiki: Option<String>,
    pub root_path: Option<String>,
    // confluence
    pub space: Option<String>,
    pub parent_id: Option<String>,
    pub title_prefix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Github,
    AzureDevops,
    Confluence,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::Github => "github",
            Target::AzureDevops => "azure-devops",
            Target::Confluence => "confluence",
        }
    }

    fn script(self) -> &'static str {
        match self {
            Target::Github => "publish_github_wiki.py",
            Target::AzureDevops => "publish_azure_devops_wiki.py",
            Target::Confluence => "publish_confluence.py",
        }
    }

    /// The Python modules its publisher imports beyond the standard library.
    fn modules(self) -> &'static [&'static str] {
        match self {
            Target::Github => &["yaml"],
            Target::AzureDevops => &["yaml", "requests"],
            Target::Confluence => &["yaml", "requests", "markdown"],
        }
    }

    /// The variables its publisher reads its credentials from (names only).
    fn credentials(self) -> &'static str {
        match self {
            Target::Github => "GITHUB_TOKEN",
            Target::AzureDevops => "AZDO_PAT",
            Target::Confluence => {
                "CONFLUENCE_URL with CONFLUENCE_PAT, or CONFLUENCE_USER and CONFLUENCE_API_TOKEN"
            }
        }
    }
}

/// Pure: the publisher script and its arguments for `docs`, or what is missing.
pub fn publisher_args(
    s: &DocsSettings,
    docs: &Path,
    dry_run: bool,
) -> anyhow::Result<(&'static str, Vec<String>)> {
    let Some(target) = s.target else {
        anyhow::bail!(
            "no documentation store: set `target` (github, azure-devops or confluence) in \
             [extensions.lakehouse.docs]"
        );
    };
    let mut args = vec!["--docs".to_string(), docs.display().to_string()];
    let mut missing = Vec::new();
    let mut need = |key: &str, flag: &str, v: &Option<String>| match v {
        Some(v) if !v.trim().is_empty() => args.extend([flag.to_string(), v.clone()]),
        _ => missing.push(key.to_string()),
    };
    match target {
        Target::Github => need("repo", "--repo", &s.repo),
        Target::AzureDevops => {
            need("org-url", "--org-url", &s.org_url);
            need("project", "--project", &s.project);
            need("wiki", "--wiki", &s.wiki);
        }
        Target::Confluence => need("space", "--space", &s.space),
    }
    if !missing.is_empty() {
        anyhow::bail!(
            "{} needs {} in [extensions.lakehouse.docs]",
            target.name(),
            missing.join(", ")
        );
    }
    let optional = [
        ("--host", &s.host, Target::Github),
        ("--prefix", &s.prefix, Target::Github),
        ("--sidebar", &s.sidebar, Target::Github),
        ("--root-path", &s.root_path, Target::AzureDevops),
        ("--parent-id", &s.parent_id, Target::Confluence),
        ("--title-prefix", &s.title_prefix, Target::Confluence),
    ];
    for (flag, v, t) in optional {
        if let (true, Some(v)) = (t == target, v) {
            args.extend([flag.to_string(), v.clone()]);
        }
    }
    if dry_run {
        args.push("--dry-run".into());
    }
    Ok((target.script(), args))
}

/// Pure: where a publish goes, in words.
pub fn destination(s: &DocsSettings) -> Option<String> {
    let t = s.target?;
    Some(match t {
        Target::Github => format!(
            "{}/{}/wiki{}",
            s.host
                .as_deref()
                .unwrap_or("https://github.com")
                .trim_end_matches('/'),
            s.repo.as_deref().unwrap_or("?"),
            s.prefix
                .as_deref()
                .map(|p| format!(" (pages prefixed {p})"))
                .unwrap_or_default()
        ),
        Target::AzureDevops => format!(
            "{}/{}/_wiki/wikis/{}{}",
            s.org_url.as_deref().unwrap_or("?").trim_end_matches('/'),
            s.project.as_deref().unwrap_or("?"),
            s.wiki.as_deref().unwrap_or("?"),
            s.root_path.as_deref().unwrap_or("")
        ),
        Target::Confluence => format!(
            "Confluence space {}{}",
            s.space.as_deref().unwrap_or("?"),
            s.parent_id
                .as_deref()
                .map(|p| format!(", under page {p}"))
                .unwrap_or_default()
        ),
    })
}

/// One publish or dry run, as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    pub target: String,
    pub destination: String,
    pub at: String,
    pub dry_run: bool,
    pub ok: bool,
    /// The generation it published (the estate record's `at`).
    pub generated_at: String,
    pub pages: u64,
    /// The publisher's last line.
    pub summary: String,
}

/// Pure: step 2 from the latest generation and the latest real publish.
pub fn step(record: Option<&Value>, published: Option<&Published>) -> Step {
    let Some(r) = record else {
        return Step::new(
            2,
            StepState::Todo,
            "generate the source-estate documentation: Generate on the Documentation page, or \
             `lakehouse docs generate`",
        );
    };
    let pages = r["pages"].as_u64().unwrap_or(0);
    let at = r["at"].as_str().unwrap_or("?");
    match published {
        Some(p) if p.ok && !p.dry_run && p.generated_at == at => Step::new(
            2,
            StepState::Done,
            format!(
                "{pages} page(s) generated {at}, published {} to {}",
                p.at, p.destination
            ),
        ),
        Some(p) if p.ok && !p.dry_run => Step::new(
            2,
            StepState::Partial,
            format!(
                "{pages} page(s) generated {at}, after the last publish ({}): publish again",
                p.at
            ),
        ),
        _ => Step::new(
            2,
            StepState::Partial,
            format!(
                "{pages} page(s) generated {at} into {}; not published",
                r["output"].as_str().unwrap_or("?")
            ),
        ),
    }
}

/// `<state>/lakehouse/docs/`.
pub fn state_dir(config: &ekos_compiler_core::EkosConfig, cwd: &Path) -> PathBuf {
    config.ekos_dir(cwd).join("lakehouse/docs")
}

pub fn read_record(config: &ekos_compiler_core::EkosConfig, cwd: &Path) -> Option<Value> {
    std::fs::read_to_string(config.ekos_dir(cwd).join(ESTATE_RECORD))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

pub fn read_published(dir: &Path, dry_run: bool) -> Option<Published> {
    std::fs::read_to_string(dir.join(if dry_run {
        "dry-run.json"
    } else {
        "publish.json"
    }))
    .ok()
    .and_then(|t| serde_json::from_str(&t).ok())
}

/// The output directory: `output` under the project directory (a home project's own directory,
/// else the workspace).
fn output_dir(ctx: &CommandContext<'_>, s: &DocsSettings) -> PathBuf {
    let base = ctx
        .config
        .location
        .as_ref()
        .map(|l| l.dir())
        .unwrap_or_else(|| ctx.cwd.to_path_buf());
    base.join(s.output.as_deref().unwrap_or("docs/estate"))
}

/// Whether the publisher can run: the interpreter and the modules its target needs.
fn check(python: &str, target: Option<Target>) -> Value {
    let modules = target.map(Target::modules).unwrap_or(&["yaml"]);
    let code = format!(
        "import importlib.util,sys; m=[x for x in {:?} if importlib.util.find_spec(x) is None]; \
         print(sys.version.split()[0]); print(','.join(m))",
        modules
    );
    match std::process::Command::new(python)
        .args(["-c", &code])
        .output()
    {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            let mut lines = out.lines();
            let version = lines.next().unwrap_or("").to_string();
            let missing: Vec<&str> = lines
                .next()
                .unwrap_or("")
                .split(',')
                .filter(|s| !s.is_empty())
                .collect();
            json!({
                "ok": missing.is_empty(),
                "text": if missing.is_empty() {
                    format!("{python} {version}: ready")
                } else {
                    format!("{python} {version}: missing {}", missing.join(", "))
                },
            })
        }
        _ => json!({ "ok": false, "text": format!("{python}: not found") }),
    }
}

/// The page's Home: `index.md` without its front matter and without the manual-section markers
/// (HTML comments, which the console shows as text).
fn home_page(dir: &Path) -> String {
    let text = std::fs::read_to_string(dir.join("index.md")).unwrap_or_default();
    let mut body = match text
        .strip_prefix("---\n")
        .and_then(|r| r.split_once("\n---\n"))
    {
        Some((_, body)) => body.trim_start().to_string(),
        None => text,
    };
    for open in ["<!-- ekos:manual", "<!-- /ekos:manual"] {
        while let Some(i) = body.find(open) {
            let end = body[i..].find("-->").map_or(body.len(), |j| i + j + 3);
            body.replace_range(i..end, "");
        }
    }
    body
}

/// `lakehouse docs show|generate|publish`.
pub fn cmd(ctx: &CommandContext<'_>, m: &clap::ArgMatches) -> anyhow::Result<()> {
    let settings = crate::settings::load(ctx.config)?;
    let s = settings.docs.clone();
    let dir = state_dir(ctx.config, ctx.cwd);
    let python = s.python.clone().unwrap_or_else(|| "python3".into());
    match m.subcommand() {
        Some(("show", m)) => {
            let record = read_record(ctx.config, ctx.cwd);
            let published = read_published(&dir, false);
            let dry = read_published(&dir, true);
            let st = step(record.as_ref(), published.as_ref());
            let out = record
                .as_ref()
                .and_then(|r| r["output"].as_str().map(PathBuf::from))
                .unwrap_or_else(|| output_dir(ctx, &s));
            let ready = check(&python, s.target);
            let line = |p: &Option<Published>| {
                p.as_ref().map(|p| {
                    format!(
                        "{} {} — {} page(s) of {} — {}",
                        if p.ok { "ok" } else { "failed" },
                        p.at,
                        p.pages,
                        p.generated_at,
                        p.summary
                    )
                })
            };
            let state = serde_json::to_value(st.state)?;
            let v = json!({
                "state": state,
                "detail": st.detail,
                "summary": {
                    "state": state,
                    "pages": record.as_ref().map(|r| r["pages"].clone()),
                    "generated_at": record.as_ref().map(|r| r["at"].clone()),
                    "output": out.display().to_string(),
                    "target": s.target.map(Target::name),
                    "destination": destination(&s),
                    "credentials": s.target.map(Target::credentials),
                    "published": line(&published),
                    "dry_run": line(&dry),
                    "publisher": ready["text"],
                },
                "home": home_page(&out),
            });
            if m.get_flag("json") {
                println!("{v}");
            } else {
                println!("step 2: {} — {}", state.as_str().unwrap_or("?"), st.detail);
                for (k, val) in v["summary"].as_object().into_iter().flatten() {
                    if !val.is_null() {
                        println!(
                            "  {k}: {}",
                            val.as_str().map(String::from).unwrap_or(val.to_string())
                        );
                    }
                }
            }
            Ok(())
        }
        Some(("generate", _)) => {
            let out = output_dir(ctx, &s);
            ekos::commands::docs_estate::generate(ctx.config, ctx.cwd, &out)
        }
        Some(("publish", m)) => {
            let dry_run = m.get_flag("dry-run");
            let Some(record) = read_record(ctx.config, ctx.cwd) else {
                anyhow::bail!("nothing generated yet: `lakehouse docs generate` first");
            };
            let docs = PathBuf::from(record["output"].as_str().unwrap_or_default());
            anyhow::ensure!(
                docs.join("index.md").is_file(),
                "{} holds no generated pages: generate again",
                docs.display()
            );
            let (script, args) = publisher_args(&s, &docs, dry_run)?;
            let target = s.target.expect("checked by publisher_args");
            let ready = check(&python, Some(target));
            anyhow::ensure!(
                ready["ok"].as_bool() == Some(true),
                "the publisher cannot run: {}",
                ready["text"].as_str().unwrap_or("")
            );
            let tools = dir.join("publish");
            std::fs::create_dir_all(&tools)?;
            for (name, text) in PUBLISHERS {
                std::fs::write(tools.join(name), text)?;
            }
            println!(
                "{} {} page(s) to {} with {script}{}",
                if dry_run { "dry run:" } else { "publishing" },
                record["pages"],
                destination(&s).unwrap_or_default(),
                if dry_run { " (nothing is sent)" } else { "" }
            );
            let o = std::process::Command::new(&python)
                .arg(tools.join(script))
                .args(&args)
                .output()?;
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            print!("{stdout}");
            eprint!("{stderr}");
            let summary = stdout
                .lines()
                .chain(stderr.lines())
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .chars()
                .take(300)
                .collect::<String>();
            let p = Published {
                target: target.name().into(),
                destination: destination(&s).unwrap_or_default(),
                at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                dry_run,
                ok: o.status.success(),
                generated_at: record["at"].as_str().unwrap_or_default().into(),
                pages: record["pages"].as_u64().unwrap_or(0),
                summary,
            };
            std::fs::write(
                dir.join(if dry_run {
                    "dry-run.json"
                } else {
                    "publish.json"
                }),
                serde_json::to_string_pretty(&p)? + "\n",
            )?;
            ekos_common::run_log::emit(
                "docs",
                if dry_run { "dry run" } else { "published" },
                &json!({ "target": p.target, "pages": p.pages, "ok": p.ok }),
            );
            anyhow::ensure!(p.ok, "the publisher failed: {}", p.summary);
            Ok(())
        }
        _ => anyhow::bail!("lakehouse docs: show, generate or publish"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github() -> DocsSettings {
        DocsSettings {
            target: Some(Target::Github),
            repo: Some("o/r".into()),
            prefix: Some("iDempiere".into()),
            sidebar: Some("keep".into()),
            space: Some("ignored for github".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_target_gives_its_publisher_and_only_its_own_settings() {
        let (script, args) = publisher_args(&github(), Path::new("/d"), true).unwrap();
        assert_eq!(script, "publish_github_wiki.py");
        assert_eq!(
            args,
            [
                "--docs",
                "/d",
                "--repo",
                "o/r",
                "--prefix",
                "iDempiere",
                "--sidebar",
                "keep",
                "--dry-run"
            ]
        );
        assert_eq!(
            destination(&github()).unwrap(),
            "https://github.com/o/r/wiki (pages prefixed iDempiere)"
        );
    }

    #[test]
    fn what_is_missing_is_named() {
        let e = publisher_args(&DocsSettings::default(), Path::new("/d"), false).unwrap_err();
        assert!(e.to_string().contains("set `target`"), "{e}");
        let azdo = DocsSettings {
            target: Some(Target::AzureDevops),
            project: Some("p".into()),
            ..Default::default()
        };
        let e = publisher_args(&azdo, Path::new("/d"), false).unwrap_err();
        assert_eq!(
            e.to_string(),
            "azure-devops needs org-url, wiki in [extensions.lakehouse.docs]"
        );
    }

    #[test]
    fn step_2_is_done_only_when_the_latest_generation_was_published() {
        assert_eq!(step(None, None).state, StepState::Todo);
        let r = json!({ "pages": 1060, "at": "T2", "output": "/d" });
        assert_eq!(step(Some(&r), None).state, StepState::Partial);
        let mut p = Published {
            target: "github".into(),
            destination: "w".into(),
            at: "T3".into(),
            dry_run: true,
            ok: true,
            generated_at: "T2".into(),
            pages: 1060,
            summary: String::new(),
        };
        assert_eq!(
            step(Some(&r), Some(&p)).state,
            StepState::Partial,
            "a dry run"
        );
        p.dry_run = false;
        assert_eq!(step(Some(&r), Some(&p)).state, StepState::Done);
        p.generated_at = "T1".into();
        let s = step(Some(&r), Some(&p));
        assert_eq!(s.state, StepState::Partial, "generated again since");
        assert!(s.detail.contains("publish again"), "{}", s.detail);
        p.generated_at = "T2".into();
        p.ok = false;
        assert_eq!(
            step(Some(&r), Some(&p)).state,
            StepState::Partial,
            "a failed publish"
        );
    }

    #[test]
    fn the_settings_table_reads_and_refuses_unknown_keys() {
        let s: DocsSettings =
            toml::from_str("target = \"azure-devops\"\norg-url = \"https://dev.azure.com/x\"")
                .unwrap();
        assert_eq!(s.target, Some(Target::AzureDevops));
        assert!(toml::from_str::<DocsSettings>("token = \"x\"").is_err());
    }

    #[test]
    fn the_home_page_loses_its_front_matter_and_the_manual_markers() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("index.md"),
            "---\ntitle: \"x\"\n---\n\n# Home\n<!-- ekos:manual id=\"home.purpose\" -->\nNotes.\n<!-- /ekos:manual -->\n",
        )
        .unwrap();
        assert_eq!(home_page(d.path()), "# Home\n\nNotes.\n\n");
    }
}

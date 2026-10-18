//! `lakehouse pentaho …` (RFC 0200–0204): Pentaho packages read into the deep Kettle model.
//!
//! This module is a raw-content entry point (RFC 0043): every file is checked against the
//! excluded-path globs and redacted before the model sees it.

use crate::settings::Settings;
use ekos::extension::CommandContext;
use ekos_common::redaction::{RedactionConfig, is_excluded_path, redact};
use ekos_lakehouse_kettle::{
    Category, Package, SourceFile, is_package_file, is_plugin_jar, read_package,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A file larger than this is skipped with a diagnostic (a `.ktr` is rarely over 1 MB).
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// A package to read: its name and directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDir {
    pub name: String,
    pub path: PathBuf,
}

/// The packages of a project: `[extensions.lakehouse] pentaho-packages` when set, otherwise
/// every observed directory (one per `[[sources]]` entry) that holds a `.ktr` or `.kjb`.
pub fn packages(settings: &Settings, ctx: &CommandContext<'_>) -> Vec<PackageDir> {
    if !settings.pentaho_packages.is_empty() {
        return settings
            .pentaho_packages
            .iter()
            .map(|(name, p)| PackageDir {
                name: name.clone(),
                path: ctx.cwd.join(p),
            })
            .collect();
    }
    ctx.config
        .observe_bases(ctx.cwd)
        .into_iter()
        .filter(|b| has_kettle_files(&b.path))
        .map(|b| PackageDir {
            name: b.key,
            path: b.path,
        })
        .collect()
}

fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules")
}

fn has_kettle_files(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if p.is_dir() {
                if !skip_dir(&name) {
                    stack.push(p);
                }
            } else if name.ends_with(".ktr") || name.ends_with(".kjb") {
                return true;
            }
        }
    }
    false
}

/// Reads the package under `root`: package files redacted, jars listed by path only.
pub fn read_dir(root: &Path, redaction: &RedactionConfig) -> Package {
    let mut files = Vec::new();
    let mut jars = Vec::new();
    let mut skipped = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !skip_dir(&name) {
                    stack.push(p);
                }
                continue;
            }
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            if is_excluded_path(&rel, redaction) {
                continue;
            }
            if is_plugin_jar(&rel) {
                jars.push(rel);
                continue;
            }
            if !is_package_file(&rel) {
                continue;
            }
            if e.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
                skipped.push((rel, "larger than 32 MB; not read".to_string()));
                continue;
            }
            match std::fs::read_to_string(&p) {
                Ok(text) => files.push(SourceFile {
                    path: rel,
                    text: redact(&text, redaction),
                }),
                Err(err) => skipped.push((rel, format!("not readable as UTF-8 text: {err}"))),
            }
        }
    }
    let mut package = read_package(&files, &jars);
    for (path, message) in skipped {
        package
            .diagnostics
            .push(ekos_lakehouse_kettle::Diagnostic { path, message });
    }
    package.diagnostics.sort();
    package
}

/// One package's summary for `pentaho scan` (and the console's Packages panel).
pub fn summary(name: &str, path: &Path, package: &Package) -> Value {
    let c = package.coverage();
    let unknown: Vec<Value> = c
        .step_types
        .iter()
        .filter(|(_, (cat, _))| *cat == Category::Unknown)
        .map(|(t, (_, n))| json!({"type": t, "steps": n}))
        .collect();
    let categories: serde_json::Map<String, Value> = c
        .categories
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), json!(v)))
        .collect();
    json!({
        "package": name,
        "path": path.display().to_string(),
        "transformations": c.transformations,
        "jobs": c.jobs,
        "steps": c.steps,
        "step_types": c.step_types.len(),
        "entries": c.entries,
        "categories": categories,
        "unknown_types": unknown,
        "links": c.links,
        "unresolved_links": c.unresolved_links,
        "plugins": package.plugins.len(),
        "diagnostics": package.diagnostics,
    })
}

/// `lakehouse pentaho scan`.
pub fn scan(settings: &Settings, ctx: &CommandContext<'_>, as_json: bool) -> anyhow::Result<()> {
    let redaction = ctx.config.redaction_config();
    let rows: Vec<Value> = packages(settings, ctx)
        .iter()
        .map(|p| summary(&p.name, &p.path, &read_dir(&p.path, &redaction)))
        .collect();
    if as_json {
        println!("{}", serde_json::to_string(&json!({ "packages": rows }))?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "no Pentaho package found: no observed directory holds a .ktr or .kjb \
             (set [extensions.lakehouse] pentaho-packages to name one)"
        );
    }
    for r in &rows {
        println!(
            "{}: {} transformations, {} jobs, {} steps of {} types, {} links ({} unresolved)",
            r["package"].as_str().unwrap_or(""),
            r["transformations"],
            r["jobs"],
            r["steps"],
            r["step_types"],
            r["links"],
            r["unresolved_links"]
        );
        for (cat, n) in r["categories"].as_object().into_iter().flatten() {
            println!("  {cat:<20} {n}");
        }
        for d in r["diagnostics"].as_array().into_iter().flatten() {
            println!(
                "  ! {}: {}",
                d["path"].as_str().unwrap_or(""),
                d["message"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reader_redacts_and_honours_excluded_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("etl/.git")).unwrap();
        std::fs::write(
            dir.path().join("etl/load.ktr"),
            "<transformation><info><name>load</name></info>\
             <connection><name>src</name><type>POSTGRESQL</type>\
             <password>Encrypted 2be98afc86aa7b88bb81cbe788fc0fce5</password></connection>\
             <step><name>JS</name><type>ScriptValueMod</type>\
             <jsScript_script>var token = \"ghp_abcdefghijklmnopqrstuvwxyz0123456789\";</jsScript_script></step>\
             </transformation>",
        )
        .unwrap();
        std::fs::write(dir.path().join("etl/.git/x.ktr"), "<transformation/>").unwrap();
        std::fs::write(dir.path().join("etl/.env"), "PASSWORD=x").unwrap();
        let p = read_dir(dir.path(), &RedactionConfig::default());
        assert_eq!(p.transformations.len(), 1, "hidden directories are skipped");
        let all = serde_json::to_string(&p).unwrap();
        assert!(!all.contains("2be98afc"), "{all}");
        assert!(!all.contains("ghp_abcdefghij"), "{all}");
        let s = summary("etl", dir.path(), &p);
        assert_eq!(s["categories"]["code"], 1);
    }
}

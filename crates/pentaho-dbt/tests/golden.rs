//! RFC 0266's golden gate: the Pentaho translator's output on a corpus, as a manifest of hashes,
//! must not change when the expression and relational IR move into the engine-neutral crates.
//!
//! The manifest holds one line per written file (path and SHA-256), one for the report, and one
//! per transformation's IR (its JSON). Hashes only: the corpora are third-party and never vendored.
//!
//! - The in-repo corpus (`crates/lakehouse-ext/examples/bench-gold`) is always checked, by every
//!   `cargo test --workspace`.
//! - `EKOS_PENTAHO_GOLDEN_CORPORA=<name>=<dir>[:<name>=<dir>…]` adds local corpora (Pentaho's
//!   samples, AdventureWorks).
//! - `EKOS_PENTAHO_GOLDEN=capture` writes the manifests instead of comparing them.

use ekos_lakehouse_bronze::plan::{PlanSettings, plan};
use ekos_lakehouse_kettle::{SourceFile, read_package};
use ekos_lakehouse_pentaho_dbt::{Settings, build_irs, translate};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn kettle_files(root: &Path) -> Vec<SourceFile> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != ".git") {
                    stack.push(p);
                }
                continue;
            }
            let lower = p.to_string_lossy().to_ascii_lowercase();
            if !(lower.ends_with(".ktr") || lower.ends_with(".kjb")) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&p) else {
                continue;
            };
            out.push(SourceFile {
                path: p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                text: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn manifest(name: &str, root: &Path) -> String {
    let package = read_package(&kettle_files(root), &[]);
    let plan = plan(&[], &PlanSettings::default());
    let settings = Settings {
        package: name.into(),
        empty_string_is_null: true,
        ..Settings::default()
    };
    let t = translate(&package, &plan, &settings);
    let mut lines: Vec<String> = t
        .files
        .iter()
        .map(|(path, text)| format!("file {path} {}", sha(text.as_bytes())))
        .collect();
    lines.push(format!(
        "report {}",
        sha(serde_json::to_string(&t.report).unwrap().as_bytes())
    ));
    for (tr, ir) in build_irs(&package, &plan, &settings) {
        lines.push(format!(
            "ir {} {}",
            tr.path,
            sha(serde_json::to_string(&ir).unwrap().as_bytes())
        ));
    }
    lines.join("\n") + "\n"
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn corpora() -> Vec<(String, PathBuf)> {
    let mut out = vec![(
        "bench-gold".to_string(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../lakehouse-ext/examples/bench-gold"),
    )];
    if let Ok(extra) = std::env::var("EKOS_PENTAHO_GOLDEN_CORPORA") {
        for item in extra.split(':').filter(|s| !s.is_empty()) {
            let (name, dir) = item.split_once('=').expect("<name>=<dir>");
            out.push((name.to_string(), PathBuf::from(dir)));
        }
    }
    out
}

#[test]
fn translations_match_their_golden_manifests() {
    let capture = std::env::var("EKOS_PENTAHO_GOLDEN").as_deref() == Ok("capture");
    std::fs::create_dir_all(golden_dir()).unwrap();
    for (name, dir) in corpora() {
        let got = manifest(&name, &dir);
        let path = golden_dir().join(format!("{name}.txt"));
        if capture {
            std::fs::write(&path, &got).unwrap();
            eprintln!(
                "captured {} ({} lines)",
                path.display(),
                got.lines().count()
            );
            continue;
        }
        let Ok(want) = std::fs::read_to_string(&path) else {
            eprintln!(
                "skipped {name}: no manifest at {} (capture it first)",
                path.display()
            );
            continue;
        };
        if got != want {
            let diff: Vec<String> = got
                .lines()
                .zip(want.lines())
                .filter(|(g, w)| g != w)
                .take(10)
                .map(|(g, w)| format!("  got  {g}\n  want {w}"))
                .collect();
            panic!(
                "{name}: the translation changed ({} vs {} lines)\n{}",
                got.lines().count(),
                want.lines().count(),
                diff.join("\n")
            );
        }
        eprintln!("{name}: {} lines identical", got.lines().count());
    }
}

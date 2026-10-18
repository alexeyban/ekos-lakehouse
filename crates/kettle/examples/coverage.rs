//! Prints a package's coverage: `cargo run -p ekos-lakehouse-kettle --example coverage -- <dir>`.
//! A measuring tool only: it does not redact, so never point it at files that hold secrets you
//! would not print. `lakehouse pentaho scan` is the real entry point.

use ekos_lakehouse_kettle::{SourceFile, is_package_file, is_plugin_jar, read_package};
use std::path::Path;

fn walk(root: &Path, dir: &Path, files: &mut Vec<SourceFile>, jars: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, files, jars);
            continue;
        }
        let rel = p
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if is_plugin_jar(&rel) {
            jars.push(rel);
        } else if is_package_file(&rel)
            && let Ok(text) = std::fs::read_to_string(&p)
        {
            files.push(SourceFile { path: rel, text });
        }
    }
}

fn main() {
    let root = std::env::args().nth(1).expect("usage: coverage <dir>");
    let root = Path::new(&root);
    let (mut files, mut jars) = (Vec::new(), Vec::new());
    walk(root, root, &mut files, &mut jars);
    let p = read_package(&files, &jars);
    let c = p.coverage();
    println!(
        "{} transformations, {} jobs, {} steps ({} types), {} entries, {} links ({} unresolved), {} diagnostics",
        c.transformations,
        c.jobs,
        c.steps,
        c.step_types.len(),
        c.entries,
        c.links,
        c.unresolved_links,
        c.diagnostics
    );
    for (cat, n) in &c.categories {
        println!("  {:<20} {n}", cat.as_str());
    }
    let unknown: Vec<_> = c
        .step_types
        .iter()
        .filter(|(_, (cat, _))| *cat == ekos_lakehouse_kettle::Category::Unknown)
        .map(|(t, (_, n))| format!("{t}×{n}"))
        .collect();
    println!("  unknown types: {}", unknown.join(", "));
    for d in p.diagnostics.iter().take(15) {
        println!("  ! {}: {}", d.path, d.message);
    }
}

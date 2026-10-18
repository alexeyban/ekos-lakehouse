//! IR coverage of a package without a catalog:
//! `cargo run -p ekos-lakehouse-kettle-rel --example ir_coverage -- <dir>`. Measuring only: it
//! does not redact (see `lakehouse pentaho scan`).

use ekos_lakehouse_kettle::{SourceFile, is_package_file, read_package};
use ekos_lakehouse_kettle_expr::Fidelity;
use ekos_lakehouse_kettle_rel::{NoCatalog, Op, build};
use std::collections::BTreeMap;
use std::path::Path;

fn walk(root: &Path, dir: &Path, files: &mut Vec<SourceFile>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, files);
        } else {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if is_package_file(&rel)
                && let Ok(text) = std::fs::read_to_string(&p)
            {
                files.push(SourceFile { path: rel, text });
            }
        }
    }
}

fn main() {
    let root = std::env::args().nth(1).expect("usage: ir_coverage <dir>");
    let mut files = Vec::new();
    walk(Path::new(&root), Path::new(&root), &mut files);
    let p = read_package(&files, &[]);
    let mut by_fid: BTreeMap<Fidelity, usize> = BTreeMap::new();
    let mut by_type: BTreeMap<String, BTreeMap<Fidelity, usize>> = BTreeMap::new();
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for t in &p.transformations {
        let ir = build(&p, t, &NoCatalog);
        for n in &ir.nodes {
            if n.step.contains('/') {
                continue; // inlined mapping steps are counted in their own file
            }
            *by_fid.entry(n.fidelity).or_default() += 1;
            *by_type
                .entry(n.step_type.clone())
                .or_default()
                .entry(n.fidelity)
                .or_default() += 1;
            if let Op::Gap { reason } = &n.op {
                let key: String = if std::env::var("ROOT_CAUSES").is_ok() {
                    if reason.starts_with("input `") || reason.contains("has no input") {
                        continue;
                    }
                    format!(
                        "{}: {}",
                        n.step_type,
                        reason.chars().take(110).collect::<String>()
                    )
                } else {
                    reason.chars().take(70).collect()
                };
                *reasons.entry(key).or_default() += 1;
            }
        }
    }
    println!("nodes by fidelity: {by_fid:?}");
    for (t, m) in &by_type {
        println!("  {t:<24} {m:?}");
    }
    let mut r: Vec<_> = reasons.into_iter().collect();
    r.sort_by_key(|x| std::cmp::Reverse(x.1));
    let limit = if std::env::var("ROOT_CAUSES").is_ok() {
        usize::MAX
    } else {
        25
    };
    for (reason, n) in r.iter().take(limit) {
        println!("  {n:>4}  {reason}");
    }
}

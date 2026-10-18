//! Prints each translated model's plannable SQL as JSON lines, for running it by hand against a
//! sandbox: `cargo run -p ekos-lakehouse-pentaho-dbt --example plannable -- <out dir> <lock>`.

use std::collections::BTreeMap;
use std::path::Path;

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else if let Ok(t) = std::fs::read_to_string(&p) {
            out.insert(
                p.strip_prefix(root).unwrap().to_string_lossy().into_owned(),
                t,
            );
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, lock) = (Path::new(&args[1]), &args[2]);
    let plan: ekos_lakehouse_bronze::Plan =
        serde_json::from_str(&std::fs::read_to_string(lock).unwrap()).unwrap();
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    for (path, body) in &files {
        if !path.ends_with(".sql") || body.contains("enabled=false") {
            continue;
        }
        let name = path.rsplit('/').next().unwrap().trim_end_matches(".sql");
        match ekos_lakehouse_pentaho_dbt::plannable_sql(name, &files, &plan) {
            Ok(sql) => println!("{}", serde_json::json!({"model": name, "sql": sql})),
            Err(e) => println!("{}", serde_json::json!({"model": name, "error": e})),
        }
    }
}

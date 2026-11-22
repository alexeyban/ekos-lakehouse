//! RFC 0266 (and 0268, for `etl-dbt`): the engine-neutral crates depend on no tool's crate — Kettle's, Pentaho's or, later,
//! Talend's — so a fix made there serves every translator.

fn dependencies(manifest: &str) -> Vec<String> {
    let text = std::fs::read_to_string(manifest).unwrap();
    let mut in_deps = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_deps = l.ends_with("dependencies]");
            continue;
        }
        if in_deps && let Some((name, _)) = l.split_once(['=', '.']) {
            out.push(name.trim().to_string());
        }
    }
    out
}

#[test]
fn the_neutral_crates_depend_on_no_tool() {
    let root = env!("CARGO_MANIFEST_DIR");
    for manifest in [
        format!("{root}/Cargo.toml"),
        format!("{root}/../etl-expr/Cargo.toml"),
        format!("{root}/../etl-dbt/Cargo.toml"),
    ] {
        for dep in dependencies(&manifest) {
            assert!(
                !["kettle", "pentaho", "talend"]
                    .iter()
                    .any(|t| dep.contains(t)),
                "{manifest} depends on {dep}"
            );
        }
    }
}

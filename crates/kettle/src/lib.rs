//! The deep Pentaho Data Integration (Kettle) model (RFC 0201): a whole package — every
//! transformation and job, their steps, hops, connections, parameters and variables, plugin
//! declarations, and the links between files — read from text the caller supplies. Pure: no
//! filesystem, no network. The caller is the raw-content entry point and redacts first
//! (RFC 0043); the model drops password elements again on its own.

pub mod catalogue;
pub mod links;
pub mod model;
pub mod parse;
pub mod variables;
pub mod xml;

pub use catalogue::{Category, EntryCategory};
pub use model::*;

use parse::XmlFile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One file of a package: its package-relative path (`/`-separated) and its redacted text.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: String,
    pub text: String,
}

/// Whether the reader wants the file at `path` (by name), so a caller reads only those.
pub fn is_package_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    lower.ends_with(".ktr")
        || lower.ends_with(".kjb")
        || lower.ends_with(".xml")
        || name == "kettle.properties"
}

/// Whether `path` is a plugin jar (listed, never read here).
pub fn is_plugin_jar(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".jar")
}

/// `a/./b/../c` → `a/c`, backslashes to `/`, no leading `./` or `/`.
pub fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let unified = path.replace('\\', "/");
    for p in unified.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

/// Reads a package from its files (any order) and the paths of its jars.
pub fn read_package(files: &[SourceFile], jar_paths: &[String]) -> Package {
    let mut package = Package::default();
    let mut files: Vec<&SourceFile> = files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for f in files {
        let path = normalize_path(&f.path);
        let lower = path.to_ascii_lowercase();
        if lower.rsplit('/').next() == Some("kettle.properties") {
            package
                .properties
                .extend(variables::parse_properties(&f.text));
            continue;
        }
        let kettle_file = lower.ends_with(".ktr") || lower.ends_with(".kjb");
        match parse::parse_xml_file(&path, &f.text) {
            Ok(XmlFile::Transformation(t)) => package.transformations.push(t),
            Ok(XmlFile::Job(j)) => package.jobs.push(j),
            Ok(XmlFile::Repository(ts, js)) => {
                package.transformations.extend(ts);
                package.jobs.extend(js);
            }
            Ok(XmlFile::Shared(cs)) => package.shared_connections.extend(cs),
            Ok(XmlFile::Plugin(p)) => package.plugins.push(p),
            Ok(XmlFile::Other) if kettle_file => package.diagnostics.push(Diagnostic {
                path,
                message: "neither a transformation nor a job".into(),
            }),
            Ok(XmlFile::Other) => {}
            // Any other XML in the tree is not ours to complain about.
            Err(e) if kettle_file => package.diagnostics.push(Diagnostic {
                path,
                message: e.to_string(),
            }),
            Err(_) => {}
        }
    }
    let mut jars: Vec<String> = jar_paths.iter().map(|j| normalize_path(j)).collect();
    jars.sort();
    for p in &mut package.plugins {
        let dir = p
            .path
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_default();
        for j in &jars {
            let beside = j.rsplit_once('/').map(|(d, _)| d).unwrap_or("") == dir;
            if beside && !p.jars.contains(j) {
                p.jars.push(j.clone());
            }
        }
        p.jars.sort();
    }
    duplicate_paths(&mut package);
    package.links = links::resolve_links(&package);
    for l in package.links.iter().filter(|l| l.to.is_none()) {
        package.diagnostics.push(Diagnostic {
            path: l.from.clone(),
            message: format!(
                "{} `{}` runs `{}`, which is not in the package",
                l.via_type, l.via, l.reference
            ),
        });
    }
    for c in links::cycles(&package.links) {
        package.diagnostics.push(Diagnostic {
            path: c[0].clone(),
            message: format!("files call each other in a cycle: {}", c.join(" → ")),
        });
    }
    package.diagnostics.sort();
    package
}

/// Two objects at one path (a file and a repository export entry): keep the first, report.
fn duplicate_paths(package: &mut Package) {
    let mut seen = std::collections::BTreeSet::new();
    let mut dups = Vec::new();
    package.transformations.retain(|t| {
        let fresh = seen.insert(t.path.clone());
        if !fresh {
            dups.push(t.path.clone());
        }
        fresh
    });
    package.jobs.retain(|j| {
        let fresh = seen.insert(j.path.clone());
        if !fresh {
            dups.push(j.path.clone());
        }
        fresh
    });
    for d in dups {
        package.diagnostics.push(Diagnostic {
            path: d,
            message: "defined twice; the first definition is used".into(),
        });
    }
}

/// Counts per step type, category and job entry type (RFC 0201 §Coverage).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Coverage {
    pub transformations: usize,
    pub jobs: usize,
    pub steps: usize,
    pub entries: usize,
    /// type id → (category, count).
    pub step_types: BTreeMap<String, (Category, usize)>,
    pub categories: BTreeMap<Category, usize>,
    pub entry_types: BTreeMap<String, (EntryCategory, usize)>,
    pub links: usize,
    pub unresolved_links: usize,
    pub diagnostics: usize,
}

impl Package {
    pub fn coverage(&self) -> Coverage {
        let mut c = Coverage {
            transformations: self.transformations.len(),
            jobs: self.jobs.len(),
            links: self.links.len(),
            unresolved_links: self.links.iter().filter(|l| l.to.is_none()).count(),
            diagnostics: self.diagnostics.len(),
            ..Coverage::default()
        };
        for s in self.transformations.iter().flat_map(|t| &t.steps) {
            c.steps += 1;
            c.step_types
                .entry(s.type_id.clone())
                .or_insert((s.category, 0))
                .1 += 1;
            *c.categories.entry(s.category).or_default() += 1;
        }
        for e in self.jobs.iter().flat_map(|j| &j.entries) {
            c.entries += 1;
            c.entry_types
                .entry(e.type_id.clone())
                .or_insert((e.category, 0))
                .1 += 1;
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> SourceFile {
        SourceFile {
            path: path.into(),
            text: text.into(),
        }
    }

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize_path("./a/b/../c.ktr"), "a/c.ktr");
        assert_eq!(normalize_path("a\\b.ktr"), "a/b.ktr");
        assert_eq!(normalize_path("/abs/x.ktr"), "abs/x.ktr");
    }

    #[test]
    fn links_resolve_by_file_and_repository_and_report_the_rest() {
        let job = r#"<job><name>main</name><entries>
            <entry><name>A</name><type>TRANS</type><filename>${Internal.Entry.Current.Directory}/sub/a.ktr</filename></entry>
            <entry><name>B</name><type>TRANS</type><specification_method>rep_name</specification_method><directory>/etl</directory><transname>b</transname></entry>
            <entry><name>C</name><type>JOB</type><filename>${ETL_HOME}/c.kjb</filename></entry>
            <entry><name>Abs</name><type>TRANS</type><filename>/home/me/project/jobs/sub/a.ktr</filename></entry>
          </entries><hops/></job>"#;
        let a = r#"<transformation><info><name>a</name></info>
            <step><name>m</name><type>Mapping</type><filename>${Internal.Transformation.Filename.Directory}/../loop.ktr</filename></step></transformation>"#;
        let lp = r#"<transformation><info><name>loop</name></info>
            <step><name>back</name><type>TransExecutor</type><filename>${Internal.Transformation.Filename.Directory}/sub/a.ktr</filename></step></transformation>"#;
        let repo = r#"<repository><transformations><transformation><info><name>b</name><directory>/etl</directory></info></transformation></transformations></repository>"#;
        let p = read_package(
            &[
                file("jobs/main.kjb", job),
                file("jobs/sub/a.ktr", a),
                file("jobs/loop.ktr", lp),
                file("export.xml", repo),
                file("notes.xml", "<html/>"),
            ],
            &[],
        );
        let to = |via: &str| p.links.iter().find(|l| l.via == via).unwrap().to.clone();
        assert_eq!(to("A").as_deref(), Some("jobs/sub/a.ktr"));
        assert_eq!(to("B").as_deref(), Some("etl/b.ktr"));
        assert_eq!(to("C"), None);
        assert_eq!(to("Abs").as_deref(), Some("jobs/sub/a.ktr"));
        assert!(
            p.diagnostics
                .iter()
                .any(|d| d.message.contains("${ETL_HOME}/c.kjb"))
        );
        assert!(
            p.diagnostics.iter().any(|d| d.message.contains("cycle")),
            "{:?}",
            p.diagnostics
        );
        let c = p.coverage();
        assert_eq!((c.transformations, c.jobs, c.unresolved_links), (3, 1, 1));
    }

    #[test]
    fn a_broken_file_does_not_stop_the_package() {
        let p = read_package(
            &[
                file("bad.ktr", "<transformation><step>"),
                file(
                    "good.ktr",
                    "<transformation><info><name>g</name></info></transformation>",
                ),
                file("kettle.properties", "A=1\n"),
            ],
            &[],
        );
        assert_eq!(p.transformations.len(), 1);
        assert_eq!(p.diagnostics.len(), 1);
        assert_eq!(p.properties["A"], "1");
    }

    #[test]
    fn plugin_jars_beside_plugin_xml_are_attached() {
        let p = read_package(
            &[file(
                "plugins/acme/plugin.xml",
                r#"<plugin id="Acme" classname="com.acme.Meta"/>"#,
            )],
            &["plugins/acme/acme.jar".into(), "plugins/other/x.jar".into()],
        );
        assert_eq!(p.plugins[0].jars, vec!["plugins/acme/acme.jar".to_string()]);
    }
}

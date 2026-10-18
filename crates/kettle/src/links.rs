//! Links between files (RFC 0201): job entries and mapping/executor steps that run another
//! transformation or job, resolved against the package.

use crate::model::{Link, LinkTarget, Package};
use crate::normalize_path;
use crate::variables::Scope;
use crate::xml::Node;

/// How one entry or step names its target, before resolution.
struct Reference<'a> {
    filename: Option<&'a str>,
    directory: Option<&'a str>,
    name: Option<&'a str>,
}

fn reference<'a>(xml: &'a Node, kind: LinkTarget) -> Reference<'a> {
    let name = match kind {
        LinkTarget::Transformation => xml
            .text_at("trans_name")
            .or_else(|| xml.text_at("transname")),
        LinkTarget::Job => xml.text_at("job_name").or_else(|| xml.text_at("jobname")),
    };
    Reference {
        filename: xml.text_at("filename").or_else(|| xml.text_at("fileName")),
        directory: xml
            .text_at("directory_path")
            .or_else(|| xml.text_at("directory")),
        name,
    }
}

/// What a step or entry of type `type_id` runs, if it runs anything.
fn target_kind(type_id: &str) -> Option<LinkTarget> {
    match type_id {
        "Mapping" | "SimpleMapping" | "TransExecutor" | "SingleThreader" | "MetaInject"
        | "TRANS" => Some(LinkTarget::Transformation),
        "JobExecutor" | "JOB" => Some(LinkTarget::Job),
        _ => None,
    }
}

/// Every link in `package`, resolved. Call after all files are read.
pub fn resolve_links(package: &Package) -> Vec<Link> {
    let mut links = Vec::new();
    for t in &package.transformations {
        let scope = file_scope(package, &t.path, &t.name, false, &t.parameters);
        for s in &t.steps {
            if let Some(kind) = target_kind(&s.type_id) {
                links.push(link(
                    package, &scope, &t.path, &s.name, &s.type_id, kind, &s.xml,
                ));
            }
        }
    }
    for j in &package.jobs {
        let scope = file_scope(package, &j.path, &j.name, true, &j.parameters);
        for e in &j.entries {
            if let Some(kind) = target_kind(&e.type_id) {
                links.push(link(
                    package, &scope, &j.path, &e.name, &e.type_id, kind, &e.xml,
                ));
            }
        }
    }
    links.sort();
    links.dedup();
    links
}

/// The variable scope of one file.
pub fn file_scope(
    package: &Package,
    path: &str,
    name: &str,
    is_job: bool,
    parameters: &[crate::model::Parameter],
) -> Scope {
    let params: Vec<_> = parameters
        .iter()
        .map(|p| (p.name.clone(), p.default.clone()))
        .collect();
    Scope::for_file(path, name, is_job, &package.properties, &params)
}

fn link(
    package: &Package,
    scope: &Scope,
    from: &str,
    via: &str,
    via_type: &str,
    kind: LinkTarget,
    xml: &Node,
) -> Link {
    let r = reference(xml, kind);
    let (reference, to) = if let Some(f) = r.filename {
        let resolved = scope.resolve(f).text;
        let to = by_filename(package, kind, &resolved);
        (resolved, to)
    } else if let Some(n) = r.name {
        let dir = r
            .directory
            .map(|d| scope.resolve(d).text)
            .unwrap_or_default();
        let name = scope.resolve(n).text;
        let to = by_repository(package, kind, &dir, &name);
        (format!("{}/{name}", dir.trim_end_matches('/')), to)
    } else {
        (String::new(), None)
    };
    Link {
        from: from.to_string(),
        via: via.to_string(),
        via_type: via_type.to_string(),
        target_kind: kind,
        to,
        reference,
    }
}

fn paths(package: &Package, kind: LinkTarget) -> Vec<&str> {
    match kind {
        LinkTarget::Transformation => package
            .transformations
            .iter()
            .map(|t| t.path.as_str())
            .collect(),
        LinkTarget::Job => package.jobs.iter().map(|j| j.path.as_str()).collect(),
    }
}

/// Exact package path first; else the one package path the reference ends with (an absolute
/// path from the author's machine). Ambiguous or missing: `None`.
fn by_filename(package: &Package, kind: LinkTarget, reference: &str) -> Option<String> {
    let wanted = normalize_path(reference.trim_start_matches("file://"));
    let all = paths(package, kind);
    if let Some(p) = all.iter().find(|p| **p == wanted) {
        return Some(p.to_string());
    }
    let suffix: Vec<_> = all
        .iter()
        .filter(|p| wanted.ends_with(&format!("/{p}")))
        .collect();
    match suffix.as_slice() {
        [one] => Some(one.to_string()),
        _ => None,
    }
}

/// Repository directory + name; else the one object with that name.
fn by_repository(package: &Package, kind: LinkTarget, dir: &str, name: &str) -> Option<String> {
    let dir = format!("/{}", dir.trim_matches('/'));
    let objects: Vec<(&str, &str, &str)> = match kind {
        LinkTarget::Transformation => package
            .transformations
            .iter()
            .map(|t| (t.path.as_str(), t.directory.as_str(), t.name.as_str()))
            .collect(),
        LinkTarget::Job => package
            .jobs
            .iter()
            .map(|j| (j.path.as_str(), j.directory.as_str(), j.name.as_str()))
            .collect(),
    };
    let norm = |d: &str| format!("/{}", d.trim_matches('/'));
    if let Some((p, _, _)) = objects
        .iter()
        .find(|(_, d, n)| norm(d) == dir && *n == name)
    {
        return Some(p.to_string());
    }
    let named: Vec<_> = objects.iter().filter(|(_, _, n)| *n == name).collect();
    match named.as_slice() {
        [one] => Some(one.0.to_string()),
        _ => None,
    }
}

/// Every cycle of resolved links, each as the file paths along it (first path repeated last).
pub fn cycles(links: &[Link]) -> Vec<Vec<String>> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut graph: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for l in links {
        if let Some(to) = &l.to {
            graph
                .entry(l.from.as_str())
                .or_default()
                .insert(to.as_str());
        }
    }
    let mut found = BTreeSet::new();
    for start in graph.keys() {
        let mut stack = vec![(*start, vec![*start])];
        while let Some((node, path)) = stack.pop() {
            for next in graph.get(node).into_iter().flatten() {
                if *next == *start {
                    let mut cycle: Vec<String> = path.iter().map(|s| s.to_string()).collect();
                    // Canonical rotation: start at the smallest path, so each cycle is listed once.
                    let min = cycle
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, p)| *p)
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    cycle.rotate_left(min);
                    cycle.push(cycle[0].clone());
                    found.insert(cycle);
                } else if !path.contains(next) {
                    let mut p = path.clone();
                    p.push(next);
                    stack.push((next, p));
                }
            }
        }
    }
    found.into_iter().collect()
}

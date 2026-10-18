//! Variable resolution (RFC 0201): `${NAME}` and `%%NAME%%`, from parameter defaults,
//! `kettle.properties` and the internal variables. Never guesses: an unknown name stays in the
//! text and is reported.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Where a variable's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Parameter,
    Properties,
    Internal,
}

/// The values available while reading one file.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    values: BTreeMap<String, (String, Source)>,
}

/// The result of resolving one string.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Resolved {
    pub text: String,
    /// `(name, source)` of every variable replaced, in order of appearance.
    pub used: Vec<(String, Source)>,
    /// Names with no value; their references are left in `text`.
    pub unresolved: Vec<String>,
}

impl Resolved {
    pub fn is_complete(&self) -> bool {
        self.unresolved.is_empty()
    }
}

impl Scope {
    /// The scope of the file at package path `path` (a `.ktr` or `.kjb`) named `name`.
    /// Lowest precedence first: internal, then properties, then the file's own parameters.
    pub fn for_file(
        path: &str,
        name: &str,
        is_job: bool,
        properties: &BTreeMap<String, String>,
        parameters: &[(String, Option<String>)],
    ) -> Scope {
        let mut scope = Scope::default();
        let dir = match path.rfind('/') {
            Some(i) => &path[..i],
            None => "",
        };
        let dir = if dir.is_empty() { "." } else { dir };
        let internal: &[(&str, &str)] = if is_job {
            &[
                ("Internal.Job.Filename.Directory", dir),
                ("Internal.Entry.Current.Directory", dir),
                ("Internal.Job.Name", name),
                ("Internal.Job.Repository.Directory", dir),
            ]
        } else {
            &[
                ("Internal.Transformation.Filename.Directory", dir),
                ("Internal.Entry.Current.Directory", dir),
                ("Internal.Transformation.Name", name),
                ("Internal.Transformation.Repository.Directory", dir),
            ]
        };
        for (k, v) in internal {
            scope.set(k, v, Source::Internal);
        }
        for (k, v) in properties {
            scope.set(k, v, Source::Properties);
        }
        for (k, v) in parameters {
            if let Some(v) = v {
                scope.set(k, v, Source::Parameter);
            }
        }
        scope
    }

    pub fn set(&mut self, name: &str, value: &str, source: Source) {
        self.values
            .insert(name.to_string(), (value.to_string(), source));
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(|(v, _)| v.as_str())
    }

    /// Replaces every `${NAME}` and `%%NAME%%` that has a value. One pass: a value is not
    /// itself resolved again (Kettle resolves once too).
    pub fn resolve(&self, text: &str) -> Resolved {
        let mut out = Resolved::default();
        let mut rest = text;
        while !rest.is_empty() {
            let dollar = rest.find("${");
            let percent = rest.find("%%");
            let (start, open, close) = match (dollar, percent) {
                (Some(d), Some(p)) if p < d => (p, "%%", "%%"),
                (Some(d), _) => (d, "${", "}"),
                (None, Some(p)) => (p, "%%", "%%"),
                (None, None) => break,
            };
            let after = &rest[start + open.len()..];
            let Some(end) = after.find(close) else { break };
            let name = &after[..end];
            out.text.push_str(&rest[..start]);
            let valid = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
            match self.values.get(name) {
                Some((v, src)) if valid => {
                    out.text.push_str(v);
                    out.used.push((name.to_string(), *src));
                }
                _ => {
                    out.text
                        .push_str(&rest[start..start + open.len() + end + close.len()]);
                    if valid && !out.unresolved.iter().any(|u| u == name) {
                        out.unresolved.push(name.to_string());
                    }
                }
            }
            rest = &after[end + close.len()..];
        }
        out.text.push_str(rest);
        out
    }
}

/// Parses `kettle.properties` (Java properties: `key=value` or `key: value`, `#`/`!` comments,
/// `\` line continuations).
pub fn parse_properties(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut pending = String::new();
    for raw in text.lines() {
        let line = raw.trim_start();
        if pending.is_empty() && (line.starts_with('#') || line.starts_with('!')) {
            continue;
        }
        if let Some(cont) = line.strip_suffix('\\') {
            pending.push_str(cont);
            continue;
        }
        pending.push_str(line);
        let entry = std::mem::take(&mut pending);
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let split = entry.find(['=', ':']).unwrap_or(entry.len());
        let key = entry[..split].trim();
        let value = entry.get(split + 1..).unwrap_or("").trim();
        if !key.is_empty() {
            out.insert(key.to_string(), value.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_in_precedence_order_and_reports_the_rest() {
        let props = parse_properties("# c\nDB_HOST=prod\nSCHEMA : dwh\nLONG=a\\\n  b\n");
        assert_eq!(props["LONG"], "ab");
        let scope = Scope::for_file(
            "etl/facts/fact_sales.ktr",
            "fact_sales",
            false,
            &props,
            &[
                ("SCHEMA".into(), Some("stage".into())),
                ("NOVAL".into(), None),
            ],
        );
        let r = scope.resolve(
            "${Internal.Transformation.Filename.Directory}/x.ktr on ${DB_HOST} in %%SCHEMA%% ${MISSING} $ {",
        );
        assert_eq!(r.text, "etl/facts/x.ktr on prod in stage ${MISSING} $ {");
        assert_eq!(r.unresolved, vec!["MISSING".to_string()]);
        assert_eq!(
            r.used.iter().map(|(_, s)| *s).collect::<Vec<_>>(),
            vec![Source::Internal, Source::Properties, Source::Parameter]
        );
        assert!(scope.resolve("${NOVAL}").unresolved == vec!["NOVAL".to_string()]);
    }

    #[test]
    fn a_file_at_the_root_has_directory_dot() {
        let scope = Scope::for_file("main.kjb", "main", true, &BTreeMap::new(), &[]);
        assert_eq!(
            scope
                .resolve("${Internal.Entry.Current.Directory}/a.ktr")
                .text,
            "./a.ktr"
        );
    }
}

//! Parsers for each file of a package (RFC 0201). Each takes the file's (already redacted)
//! text and returns model types; none touches the filesystem.

use crate::catalogue;
use crate::model::{
    Connection, Hop, HopCondition, Job, JobEntry, JobHop, Parameter, Plugin, Step, Transformation,
};
use crate::xml::Node;

/// Why one file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("not well-formed XML: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("expected a <{expected}> root element, found <{found}>")]
    Root {
        expected: &'static str,
        found: String,
    },
}

/// What one XML file of a package holds.
#[derive(Debug)]
pub enum XmlFile {
    Transformation(Transformation),
    Job(Job),
    /// A repository export: everything in it.
    Repository(Vec<Transformation>, Vec<Job>),
    Shared(Vec<Connection>),
    Plugin(Plugin),
    /// XML that is none of the above.
    Other,
}

fn parse_doc(text: &str) -> Result<roxmltree::Document<'_>, ParseError> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    Ok(roxmltree::Document::parse_with_options(text, opts)?)
}

/// Reads any XML file of a package by its root element.
pub fn parse_xml_file(path: &str, text: &str) -> Result<XmlFile, ParseError> {
    let doc = parse_doc(text)?;
    let root = Node::from_roxml(&doc, doc.root_element());
    Ok(match root.name.as_str() {
        "transformation" => XmlFile::Transformation(transformation(path, &root)),
        "job" => XmlFile::Job(job(path, &root)),
        "repository" => {
            let ts = root
                .children_named("transformations")
                .flat_map(|c| c.children_named("transformation"))
                .map(|t| {
                    let p = repository_path(t, "ktr");
                    transformation(&p, t)
                })
                .collect();
            let js = root
                .children_named("jobs")
                .flat_map(|c| c.children_named("job"))
                .map(|j| {
                    let p = repository_path(j, "kjb");
                    job(&p, j)
                })
                .collect();
            XmlFile::Repository(ts, js)
        }
        "sharedobjects" => {
            XmlFile::Shared(root.children_named("connection").map(connection).collect())
        }
        "plugin" => XmlFile::Plugin(plugin(path, &root)),
        _ => XmlFile::Other,
    })
}

/// Parses a `.ktr`.
pub fn parse_ktr(path: &str, text: &str) -> Result<Transformation, ParseError> {
    match parse_xml_file(path, text)? {
        XmlFile::Transformation(t) => Ok(t),
        _ => Err(root_error(text, "transformation")),
    }
}

/// Parses a `.kjb`.
pub fn parse_kjb(path: &str, text: &str) -> Result<Job, ParseError> {
    match parse_xml_file(path, text)? {
        XmlFile::Job(j) => Ok(j),
        _ => Err(root_error(text, "job")),
    }
}

fn root_error(text: &str, expected: &'static str) -> ParseError {
    let found = parse_doc(text)
        .map(|d| d.root_element().tag_name().name().to_string())
        .unwrap_or_default();
    ParseError::Root { expected, found }
}

/// `<directory>/<name>.<ext>` for an object inside a repository export.
fn repository_path(el: &Node, ext: &str) -> String {
    let dir = el
        .text_at("info/directory")
        .or_else(|| el.text_at("directory"))
        .unwrap_or("/");
    let name = el
        .text_at("info/name")
        .or_else(|| el.text_at("name"))
        .unwrap_or("unnamed");
    let dir = dir.trim_matches('/');
    if dir.is_empty() {
        format!("{name}.{ext}")
    } else {
        format!("{dir}/{name}.{ext}")
    }
}

fn parameters(container: Option<&Node>) -> Vec<Parameter> {
    container
        .map(|c| {
            c.children_named("parameter")
                .map(|p| Parameter {
                    name: p.text_at("name").unwrap_or_default().to_string(),
                    default: p.text_at("default_value").map(str::to_string),
                    description: p.text_at("description").map(str::to_string),
                })
                .filter(|p| !p.name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// One `<connection>` element.
pub fn connection(c: &Node) -> Connection {
    let attr = |code: &str| {
        c.child("attributes").and_then(|a| {
            a.children_named("attribute")
                .find(|x| x.text_at("code") == Some(code))
                .and_then(|x| x.text_at("attribute"))
                .map(str::to_string)
        })
    };
    Connection {
        name: c.text_at("name").unwrap_or_default().to_string(),
        db_type: c.text_at("type").unwrap_or_default().to_string(),
        access: c.text_at("access").unwrap_or_default().to_string(),
        host: c.text_at("server").map(str::to_string),
        database: c.text_at("database").map(str::to_string),
        port: c.text_at("port").map(str::to_string),
        username: c.text_at("username").map(str::to_string),
        schema: attr("PREFERRED_SCHEMA_NAME").filter(|s| !s.is_empty()),
    }
}

fn transformation(path: &str, root: &Node) -> Transformation {
    let steps = root
        .children_named("step")
        .map(|s| {
            let type_id = s.text_at("type").unwrap_or_default().to_string();
            Step {
                name: s.text_at("name").unwrap_or_default().to_string(),
                category: catalogue::category(&type_id),
                type_id,
                description: s.text_at("description").map(str::to_string),
                copies: s.text_at("copies").unwrap_or("1").to_string(),
                distribute: s.flag_at("distribute", true),
                line: s.line,
                xml: s.clone(),
            }
        })
        .collect();
    let mut hops: Vec<Hop> = root
        .at("order")
        .map(|o| {
            o.children_named("hop")
                .map(|h| Hop {
                    from: h.text_at("from").unwrap_or_default().to_string(),
                    to: h.text_at("to").unwrap_or_default().to_string(),
                    enabled: h.flag_at("enabled", true),
                    error: false,
                })
                .collect()
        })
        .unwrap_or_default();
    // An error-handling definition marks the hop from its source to its target as the error
    // stream (Kettle draws it as a normal hop in `<order>`).
    if let Some(eh) = root.child("step_error_handling") {
        for e in eh.children_named("error") {
            let (Some(src), Some(dst)) = (e.text_at("source_step"), e.text_at("target_step"))
            else {
                continue;
            };
            let enabled = e.flag_at("is_enabled", true);
            match hops.iter_mut().find(|h| h.from == src && h.to == dst) {
                Some(h) => {
                    h.error = true;
                    h.enabled &= enabled;
                }
                None => hops.push(Hop {
                    from: src.to_string(),
                    to: dst.to_string(),
                    enabled,
                    error: true,
                }),
            }
        }
    }
    Transformation {
        path: path.to_string(),
        name: root.text_at("info/name").unwrap_or_default().to_string(),
        description: root.text_at("info/description").map(str::to_string),
        directory: root.text_at("info/directory").unwrap_or("/").to_string(),
        parameters: parameters(root.at("info/parameters")),
        connections: root.children_named("connection").map(connection).collect(),
        steps,
        hops,
    }
}

fn job(path: &str, root: &Node) -> Job {
    let entries = root
        .at("entries")
        .map(|e| {
            e.children_named("entry")
                .map(|x| {
                    let type_id = x.text_at("type").unwrap_or_default().to_string();
                    JobEntry {
                        name: x.text_at("name").unwrap_or_default().to_string(),
                        category: catalogue::entry_category(&type_id),
                        type_id,
                        copy: x.text_at("nr").and_then(|n| n.parse().ok()).unwrap_or(0),
                        line: x.line,
                        xml: x.clone(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let hops = root
        .at("hops")
        .map(|h| {
            h.children_named("hop")
                .map(|x| JobHop {
                    from: x.text_at("from").unwrap_or_default().to_string(),
                    to: x.text_at("to").unwrap_or_default().to_string(),
                    enabled: x.flag_at("enabled", true),
                    condition: if x.flag_at("unconditional", false) {
                        HopCondition::Unconditional
                    } else if x.flag_at("evaluation", true) {
                        HopCondition::OnSuccess
                    } else {
                        HopCondition::OnFailure
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    Job {
        path: path.to_string(),
        name: root.text_at("name").unwrap_or_default().to_string(),
        description: root.text_at("description").map(str::to_string),
        directory: root.text_at("directory").unwrap_or("/").to_string(),
        parameters: parameters(root.child("parameters")),
        connections: root.children_named("connection").map(connection).collect(),
        entries,
        hops,
    }
}

fn plugin(path: &str, root: &Node) -> Plugin {
    let attr = |n: &Node, k: &str| {
        n.attrs
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let jars = root
        .child("libraries")
        .map(|l| {
            l.children_named("library")
                .map(|lib| attr(lib, "name"))
                .filter(|n| !n.is_empty())
                .map(|n| {
                    crate::normalize_path(&if dir.is_empty() {
                        n.clone()
                    } else {
                        format!("{dir}/{n}")
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Plugin {
        id: attr(root, "id"),
        class: attr(root, "classname"),
        jars,
        path: path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KTR: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<transformation>
  <info><name>fact_sales</name><directory>/</directory>
    <parameters><parameter><name>RUN_DATE</name><default_value>2020-01-01</default_value><description/></parameter></parameters>
  </info>
  <connection><name>src</name><server>db</server><type>POSTGRESQL</type><access>Native</access>
    <database>shop</database><port>5432</port><username>etl</username><password>Encrypted 2be98afc86aa7b88bb81cbe788fc0fce5</password>
    <attributes><attribute><code>PREFERRED_SCHEMA_NAME</code><attribute>sales</attribute></attribute></attributes>
  </connection>
  <order>
    <hop><from>in</from><to>calc</to><enabled>Y</enabled></hop>
    <hop><from>calc</from><to>out</to><enabled>Y</enabled></hop>
    <hop><from>calc</from><to>bad rows</to><enabled>Y</enabled></hop>
    <hop><from>old</from><to>out</to><enabled>N</enabled></hop>
  </order>
  <step><name>in</name><type>TableInput</type><connection>src</connection><sql>SELECT * FROM orders</sql><copies>1</copies></step>
  <step><name>calc</name><type>Calculator</type></step>
  <step><name>out</name><type>TableOutput</type></step>
  <step><name>bad rows</name><type>Dummy</type></step>
  <step><name>old</name><type>com.acme.Legacy</type></step>
  <step_error_handling><error><source_step>calc</source_step><target_step>bad rows</target_step><is_enabled>Y</is_enabled></error></step_error_handling>
</transformation>"#;

    #[test]
    fn reads_a_transformation_without_its_password() {
        let t = parse_ktr("etl/fact_sales.ktr", KTR).unwrap();
        assert_eq!(t.name, "fact_sales");
        assert_eq!(t.parameters[0].default.as_deref(), Some("2020-01-01"));
        let c = &t.connections[0];
        assert_eq!(
            (c.db_type.as_str(), c.schema.as_deref()),
            ("POSTGRESQL", Some("sales"))
        );
        assert!(!serde_json::to_string(&t).unwrap().contains("2be98afc"));
        assert_eq!(
            t.step("old").unwrap().category,
            catalogue::Category::Unknown
        );
        assert_eq!(t.inputs_of("out"), vec!["calc"], "disabled hop ignored");
        assert_eq!(
            t.outputs_of("calc"),
            vec!["out"],
            "error hop is not a data output"
        );
        assert!(t.hops.iter().any(|h| h.to == "bad rows" && h.error));
        assert_eq!(
            t.step("in").unwrap().xml.text_at("sql"),
            Some("SELECT * FROM orders")
        );
    }

    #[test]
    fn reads_a_job_and_its_hop_conditions() {
        let kjb = r#"<job><name>nightly</name>
          <entries>
            <entry><name>Start</name><type>SPECIAL</type><nr>0</nr></entry>
            <entry><name>Load</name><type>TRANS</type><filename>${Internal.Entry.Current.Directory}/a.ktr</filename><nr>0</nr></entry>
            <entry><name>Mail</name><type>MAIL</type><nr>0</nr></entry>
          </entries>
          <hops>
            <hop><from>Start</from><to>Load</to><enabled>Y</enabled><evaluation>Y</evaluation><unconditional>Y</unconditional></hop>
            <hop><from>Load</from><to>Mail</to><enabled>Y</enabled><evaluation>N</evaluation><unconditional>N</unconditional></hop>
          </hops></job>"#;
        let j = parse_kjb("nightly.kjb", kjb).unwrap();
        assert_eq!(j.entries.len(), 3);
        assert_eq!(j.hops[0].condition, HopCondition::Unconditional);
        assert_eq!(j.hops[1].condition, HopCondition::OnFailure);
        assert_eq!(j.entries[2].category, catalogue::EntryCategory::External);
    }

    #[test]
    fn reads_a_repository_export_at_virtual_paths() {
        let xml = r#"<repository>
          <transformations><transformation><info><name>t1</name><directory>/etl/dims</directory></info></transformation></transformations>
          <jobs><job><name>j1</name><directory>/</directory></job></jobs></repository>"#;
        let XmlFile::Repository(ts, js) = parse_xml_file("export.xml", xml).unwrap() else {
            panic!()
        };
        assert_eq!(ts[0].path, "etl/dims/t1.ktr");
        assert_eq!(js[0].path, "j1.kjb");
    }

    #[test]
    fn reads_shared_connections_and_plugins() {
        let shared = r#"<sharedobjects><connection><name>dwh</name><type>POSTGRESQL</type><password>x</password></connection></sharedobjects>"#;
        let XmlFile::Shared(c) = parse_xml_file("shared.xml", shared).unwrap() else {
            panic!()
        };
        assert_eq!(c[0].name, "dwh");
        let plugin = r#"<plugin id="AcmeStep" classname="com.acme.AcmeStepMeta"><libraries><library name="lib/acme.jar"/></libraries></plugin>"#;
        let XmlFile::Plugin(p) = parse_xml_file("plugins/acme/plugin.xml", plugin).unwrap() else {
            panic!()
        };
        assert_eq!(
            (p.id.as_str(), p.class.as_str()),
            ("AcmeStep", "com.acme.AcmeStepMeta")
        );
        assert_eq!(p.jars, vec!["plugins/acme/lib/acme.jar".to_string()]);
    }

    #[test]
    fn a_broken_file_is_an_error_not_a_panic() {
        assert!(parse_ktr("x.ktr", "<transformation><step>").is_err());
        assert!(matches!(
            parse_ktr("x.ktr", "<job/>"),
            Err(ParseError::Root {
                expected: "transformation",
                ..
            })
        ));
    }
}

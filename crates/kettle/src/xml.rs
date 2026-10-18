//! An owned element tree, so later stages read typed values without re-parsing the file.

use serde::{Deserialize, Serialize};

/// One XML element: its name, attributes, own text (concatenated text and CDATA children,
/// untrimmed — code steps keep their whitespace), child elements and the line it starts on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
    pub line: u32,
}

/// True for an element whose text is a secret: dropped from the tree whatever it holds.
pub fn is_secret_element(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("password") || n.contains("passwd")
}

impl Node {
    /// Builds the tree under `el`, dropping secret elements and the GUI layout.
    pub fn from_roxml(doc: &roxmltree::Document<'_>, el: roxmltree::Node<'_, '_>) -> Node {
        let mut node = Node {
            name: el.tag_name().name().to_string(),
            attrs: el
                .attributes()
                .map(|a| (a.name().to_string(), a.value().to_string()))
                .collect(),
            line: doc.text_pos_at(el.range().start).row,
            ..Node::default()
        };
        for child in el.children() {
            if child.is_text() {
                node.text.push_str(child.text().unwrap_or_default());
            } else if child.is_element() {
                let name = child.tag_name().name();
                if is_secret_element(name) || name == "GUI" {
                    continue;
                }
                node.children.push(Node::from_roxml(doc, child));
            }
        }
        if node.children.is_empty() {
            // Keep code verbatim; a container's text is only indentation.
        } else if node.text.trim().is_empty() {
            node.text.clear();
        }
        node
    }

    /// The first child named `name`.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Every child named `name`.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The node at a `/`-separated path of child names, e.g. `info/parameters`.
    pub fn at(&self, path: &str) -> Option<&Node> {
        path.split('/')
            .filter(|p| !p.is_empty())
            .try_fold(self, |n, p| n.child(p))
    }

    /// The trimmed text at `path`, `None` when the element is missing or empty.
    pub fn text_at(&self, path: &str) -> Option<&str> {
        self.at(path)
            .map(|n| n.text.trim())
            .filter(|t| !t.is_empty())
    }

    /// The untrimmed text at `path` (code, SQL), `None` when missing or blank.
    pub fn raw_at(&self, path: &str) -> Option<&str> {
        self.at(path)
            .map(|n| n.text.as_str())
            .filter(|t| !t.trim().is_empty())
    }

    /// Kettle's `Y`/`N` flags (also `true`/`false`); `default` when absent.
    pub fn flag_at(&self, path: &str, default: bool) -> bool {
        match self.text_at(path) {
            Some(v) => matches!(v, "Y" | "y" | "true" | "TRUE" | "True"),
            None => default,
        }
    }

    /// Every element under this one, depth first, this one included.
    pub fn descendants(&self) -> Vec<&Node> {
        let mut out = vec![self];
        let mut i = 0;
        while i < out.len() {
            let n = out[i];
            out.extend(n.children.iter());
            i += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> Node {
        let doc = roxmltree::Document::parse(xml).unwrap();
        Node::from_roxml(&doc, doc.root_element())
    }

    #[test]
    fn keeps_code_verbatim_and_drops_secrets_and_layout() {
        let n = parse(
            "<step>\n  <name>JS</name>\n  <password>x</password><httpPassword>y</httpPassword>\
             <script>  var a = 1;\n</script><GUI><xloc>1</xloc></GUI><flag>Y</flag></step>",
        );
        assert_eq!(n.text, "");
        assert_eq!(n.text_at("name"), Some("JS"));
        assert_eq!(n.raw_at("script"), Some("  var a = 1;\n"));
        assert!(n.child("password").is_none() && n.child("httpPassword").is_none());
        assert!(n.child("GUI").is_none());
        assert!(n.flag_at("flag", false) && !n.flag_at("missing", false));
        assert_eq!(n.child("script").unwrap().line, 3);
    }
}

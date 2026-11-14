//! EKOS RFC 0260 §2.1 — which terms of a mart read a source column. Pure: a mart's requirement
//! (as written in `marts/<mart>.md`) against one changed `table.column` (or the whole table).

use crate::MartSpec;
use serde::Serialize;

/// One term of a mart that names the changed column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermReading {
    pub mart: String,
    /// `dimension`, `measure`, `filter`, or `join` (the column is a `via` key of the term).
    pub section: &'static str,
    /// The term's name; a filter's own text.
    pub term: String,
    pub definition: String,
}

/// The `table.column` identifiers a definition names, lower case, in order.
fn identifiers(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch == '\'' {
            quoted = !quoted;
            cur.clear();
            continue;
        }
        if !quoted && (ch.is_ascii_alphanumeric() || ch == '_' || ch == '.') {
            cur.push(ch.to_ascii_lowercase());
        } else {
            let t = cur.trim_matches('.');
            if t.contains('.') {
                out.push(t.to_string());
            }
            cur.clear();
        }
    }
    out
}

/// Whether `ident` (`table.column`, maybe `schema.table.column`) is the changed column, or any
/// column of the changed table when `column` is `None`.
fn names(ident: &str, table: &str, column: Option<&str>) -> bool {
    let parts: Vec<&str> = ident.split('.').collect();
    let n = parts.len();
    if n < 2 || parts[n - 2] != table {
        return false;
    }
    column.is_none_or(|c| parts[n - 1] == c)
}

/// The terms of `spec` that read `table.column` (`column: None`: any column of `table`). `table`
/// may carry a schema (`adempiere.c_invoice`); names compare case-insensitively.
pub fn terms_reading(spec: &MartSpec, table: &str, column: Option<&str>) -> Vec<TermReading> {
    let table = table
        .rsplit('.')
        .next()
        .unwrap_or(table)
        .to_ascii_lowercase();
    let column = column.map(str::to_ascii_lowercase);
    let hit = |text: &str| {
        identifiers(text)
            .iter()
            .any(|i| names(i, &table, column.as_deref()))
    };
    let mut out = vec![];
    let terms = spec
        .dimensions
        .iter()
        .map(|t| ("dimension", t))
        .chain(spec.measures.iter().map(|t| ("measure", t)));
    for (section, (name, definition)) in terms {
        let lower = definition.to_ascii_lowercase();
        let (what, via) = match lower.find(" via ") {
            Some(i) => (&definition[..i], &definition[i + " via ".len()..]),
            None => (definition.as_str(), ""),
        };
        let section = if hit(what) {
            Some(section)
        } else if hit(via) {
            Some("join")
        } else {
            None
        };
        if let Some(section) = section {
            out.push(TermReading {
                mart: spec.mart.clone(),
                section,
                term: name.clone(),
                definition: definition.clone(),
            });
        }
    }
    for f in &spec.filters {
        if hit(f) {
            out.push(TermReading {
                mart: spec.mart.clone(),
                section: "filter",
                term: f.clone(),
                definition: f.clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> MartSpec {
        MartSpec {
            mart: "sales_by_category_month".into(),
            grain: vec!["month".into(), "product_category".into()],
            dimensions: vec![
                (
                    "month".into(),
                    "month of c_invoice.dateinvoiced via c_invoiceline.c_invoice_id".into(),
                ),
                (
                    "product_category".into(),
                    "m_product_category.name via c_invoiceline.m_product_id, m_product.m_product_category_id"
                        .into(),
                ),
            ],
            measures: vec![("revenue".into(), "sum of c_invoiceline.linenetamt".into())],
            filters: vec![
                "c_invoice.issotrx is true".into(),
                "c_invoice.docstatus = 'CO'".into(),
            ],
            prose: String::new(),
        }
    }

    #[test]
    fn a_dropped_column_finds_the_term_that_reads_it() {
        let r = terms_reading(&spec(), "adempiere.m_product_category", Some("NAME"));
        assert_eq!(r.len(), 1);
        assert_eq!(
            (r[0].section, r[0].term.as_str()),
            ("dimension", "product_category")
        );
    }

    #[test]
    fn whole_identifiers_only_and_never_inside_a_literal() {
        assert!(terms_reading(&spec(), "c_invoice", Some("dateinvoice")).is_empty());
        assert!(terms_reading(&spec(), "m_product_category", Some("nam")).is_empty());
        let mut s = spec();
        s.filters = vec!["c_invoice.description = 'c_order.poreference'".into()];
        assert!(terms_reading(&s, "c_order", Some("poreference")).is_empty());
    }

    #[test]
    fn a_via_key_is_a_join_and_a_filter_is_a_filter() {
        let r = terms_reading(&spec(), "m_product", Some("m_product_category_id"));
        assert_eq!(
            (r[0].section, r[0].term.as_str()),
            ("join", "product_category")
        );
        let r = terms_reading(&spec(), "c_invoice", Some("docstatus"));
        assert_eq!(
            (r[0].section, r[0].term.as_str()),
            ("filter", "c_invoice.docstatus = 'CO'")
        );
    }

    #[test]
    fn a_whole_table_change_finds_every_term_on_the_table() {
        let r = terms_reading(&spec(), "c_invoice", None);
        let got: Vec<_> = r.iter().map(|t| (t.section, t.term.as_str())).collect();
        assert_eq!(
            got,
            [
                ("dimension", "month"),
                ("filter", "c_invoice.issotrx is true"),
                ("filter", "c_invoice.docstatus = 'CO'")
            ]
        );
        assert!(terms_reading(&spec(), "c_order", None).is_empty());
    }
}

//! The checks a generated model passes before it is written enabled (EKOS RFC 0268 §Gates 2–3).

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use sqlparser::ast::{Expr, visit_expressions};

/// Java the translation must never leave in SQL: the first such text outside a comment.
pub fn raw_java(sql: &str) -> Option<String> {
    for line in sql.lines().filter(|l| !l.trim_start().starts_with("--")) {
        let code = strip_strings(line);
        for marker in ["context.", "globalMap", "input_row.", "output_row.", "Var."] {
            if let Some(at) = code.find(marker) {
                let before = code[..at].chars().next_back();
                if before.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_')) {
                    return Some(marker.trim_end_matches('.').to_string());
                }
            }
        }
        // `row1.COL`: a Talend link name used as a qualifier.
        let bytes: Vec<char> = code.chars().collect();
        for i in 0..bytes.len() {
            let starts = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == '_');
            if starts && code[byte_at(&code, i)..].starts_with("row") {
                let rest: String = bytes[i + 3..].iter().collect();
                let digits = rest.chars().take_while(char::is_ascii_digit).count();
                if digits > 0 && rest[digits..].starts_with('.') {
                    return Some(format!("row{}", &rest[..digits]));
                }
            }
        }
    }
    None
}

fn byte_at(s: &str, char_index: usize) -> usize {
    s.char_indices().nth(char_index).map_or(s.len(), |(b, _)| b)
}

/// A line with its `'…'` and `"…"` contents blanked, so a literal never counts as code.
fn strip_strings(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(qc) if c == qc => {
                quote = None;
                out.push(c);
            }
            Some(_) => out.push(' '),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                out.push(c);
            }
            None => out.push(c),
        }
    }
    out
}

/// Text from the project (a file name, a reason, commented SQL) made inert for dbt: dbt renders
/// `{{ … }}` and `{% … %}` everywhere in a file, comments and YAML descriptions included.
pub fn inert(text: &str) -> String {
    text.replace("{{", "{ {")
        .replace("}}", "} }")
        .replace("{%", "{ %")
        .replace("%}", "% }")
        .replace("{#", "{ #")
}

/// `{{ source('s', 't') }}` → `"s"."t"`, so a model's SQL parses where sources are tables.
pub fn sources_as_tables(sql: &str) -> String {
    let mut out = String::new();
    let mut rest = sql;
    while let Some(i) = rest.find("{{ source('") {
        out.push_str(&rest[..i]);
        let after = &rest[i + "{{ source('".len()..];
        let Some(j) = after.find("') }}") else {
            out.push_str(&rest[i..]);
            return out;
        };
        let inner = &after[..j];
        let (src, table) = inner.split_once("', '").unwrap_or(("talend_files", inner));
        out.push_str(&format!("\"{src}\".\"{table}\""));
        rest = &after[j + "') }}".len()..];
    }
    out.push_str(rest);
    out
}

/// A model's SQL with its Jinja taken out: `ref()` → the model name, `{{ this }}` → `this`, the
/// first-build branch of an `is_incremental()` test, config and comment lines dropped.
pub fn without_jinja(sql: &str) -> String {
    let mut out = Vec::new();
    let sql = sources_as_tables(sql);
    for line in sql.lines() {
        let t = line.trim_start();
        if t.starts_with("--") || t.starts_with("{{ config(") {
            continue;
        }
        let mut l = line.to_string();
        if let Some(i) = l.find("{% if is_incremental() %}") {
            let tail = l[i..]
                .split("{% else %}")
                .nth(1)
                .unwrap_or("")
                .replace("{% endif %}", "");
            l = format!("{}{}", &l[..i], tail);
        }
        while let Some(i) = l.find("{{ ref('") {
            let after = &l[i + "{{ ref('".len()..];
            let Some(j) = after.find("') }}") else { break };
            let model = after[..j].to_string();
            l = format!("{}{}{}", &l[..i], model, &after[j + "') }}".len()..]);
        }
        out.push(l.replace("{{ this }}", "this"));
    }
    out.join("\n")
}

/// Anti-invention: the first column name the SQL reads that is not in `known` (lower-cased). SQL
/// that does not parse is gate 1's to refuse, so it passes here.
pub fn invented(sql: &str, known: &BTreeSet<String>) -> Option<String> {
    let text = without_jinja(sql);
    let stmts = std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn_scoped(s, || {
                sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, &text)
                    .ok()
            })
            .ok()?
            .join()
            .ok()
            .flatten()
    })?;
    let mut found = None;
    let _ = visit_expressions(&stmts, |e| {
        let name = match e {
            Expr::Identifier(i) => Some(&i.value),
            Expr::CompoundIdentifier(parts) => parts.last().map(|i| &i.value),
            _ => None,
        };
        if let Some(n) = name
            && !known.contains(&n.to_ascii_lowercase())
        {
            found = Some(n.clone());
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_text_is_made_inert_for_jinja() {
        let t = inert("reads `{{(String)globalMap.get(\"k\")}}` {% if x %}");
        assert!(
            !t.contains("{{") && !t.contains("{%") && !t.contains("}}"),
            "{t}"
        );
    }

    #[test]
    fn java_left_in_sql_is_found_outside_literals_and_comments() {
        assert_eq!(
            raw_java("SELECT context.x FROM t").as_deref(),
            Some("context")
        );
        assert_eq!(
            raw_java("SELECT a FROM t WHERE row2.b = 1").as_deref(),
            Some("row2")
        );
        assert_eq!(
            raw_java("SELECT 'context.x' AS a, \"row1__ID\" FROM t"),
            None
        );
        assert_eq!(raw_java("-- context.x\nSELECT a FROM t"), None);
        assert_eq!(raw_java("SELECT my_row1.a FROM t"), None);
    }

    #[test]
    fn a_column_no_node_has_is_named() {
        let known: BTreeSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let ok =
            "{{ config(materialized='table') }}\nSELECT \"a\", l.\"b\" FROM {{ ref('m') }} AS l";
        assert_eq!(invented(ok, &known), None);
        let bad = "SELECT \"a\", \"zz\" FROM {{ ref('m') }}";
        assert_eq!(invented(bad, &known).as_deref(), Some("zz"));
        let inc = "SELECT * FROM (SELECT \"a\" FROM m) AS u\n{% if is_incremental() %}WHERE (\"a\") IN (SELECT \"a\" FROM {{ this }}){% else %}WHERE FALSE{% endif %}";
        assert_eq!(invented(inc, &known), None);
    }
}

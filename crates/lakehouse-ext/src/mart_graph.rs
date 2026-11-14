//! EKOS RFC 0257 — a gold mart drawn as a chain of steps (`lakehouse marts graph`).
//!
//! The picture is of what will run: the mart's own SQL (the committed dbt model, or what the
//! compiler would write now) is parsed by EKOS's `sql-select` front end into a Transformation IR
//! graph and drawn by `ekos_semantic::transform_chain`, the renderer `ekos transformation graph`
//! uses. The document is the same `ekos.transformation-graph/1`, with `transformation.recovered =
//! false` and each step joined back to the requirement's terms through the compiler's clause map
//! ([`ekos_lakehouse_gold::Clause`]): `term`, `rule` and `fidelity` filled, and `status:
//! unmapped` on a step no term produced — a hand edit, or something the parser did not
//! understand. Both are defects of a generated mart and are flagged, never hidden.
//!
//! Two changes to the IR before drawing, both stated in the RFC's log: a `WHERE` of several
//! `AND`ed predicates is drawn as one `Filter` step per predicate (each is one term, and each
//! removes its own rows), and the mart itself is the final `Sink` step (its grain).
//!
//! Pure: the caller reads the files.

use ekos_kir::{KirId, KirObject};
use ekos_lakehouse_gold::{self as gold, Clause, Resolved};
use ekos_semantic::transform_chain::{self, ChainGraph, Context, EvidenceView, Expand, SourceText};
use ekos_semantic::transform_ir::{
    AggExpr, NodeId, TransformGraph, TransformNode, lower_to_kir, transform_node_kir_id,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// Everything one mart's chain is drawn from.
pub struct MartSql<'a> {
    pub mart: &'a str,
    /// Where the SQL is (`dbt/models/gold/gold__<mart>.sql`), for evidence.
    pub path: &'a str,
    /// The dbt model as committed (refs and config included), or the compiler's `dbt_sql`.
    pub dbt_sql: &'a str,
    /// `committed` (what runs) or `proposed` (what the compiler would write now).
    pub version: &'a str,
    /// EKOS RFC 0259: the engine the mart is built on; its dialect parses the SQL.
    pub engine: ekos_lakehouse_gold::GoldEngine,
    pub grain: &'a [String],
    pub resolved: &'a [Resolved],
    pub clauses: &'a [Clause],
    /// RFC 0247 annotations on each silver table (lower-case name), shown on the steps that read
    /// it.
    pub notes_by_table: BTreeMap<String, Vec<Value>>,
    /// The instruction each instruction-made filter term came from, shown on its step.
    pub notes_by_term: BTreeMap<String, Vec<Value>>,
}

/// The model's SQL without its header lines, refs replaced by the model names they name
/// (`{{ ref('ADEMPIERE__C_ORDER') }}` → `"ADEMPIERE__C_ORDER"`), and the file line it starts on.
/// A DuckDB mart's silver sources name their table as the model (EKOS RFC 0259:
/// `{{ source('silver_adempiere', 'ADEMPIERE__C_ORDER') }}`), and read the same.
pub fn plain_sql(dbt: &str) -> (String, u32) {
    let mut lines = dbt.lines().enumerate().skip_while(|(_, l)| {
        let t = l.trim();
        t.is_empty() || t.starts_with("--") || t.starts_with("{{ config(")
    });
    let Some((first, line)) = lines.next() else {
        return (String::new(), 1);
    };
    let mut body = String::from(line);
    for (_, l) in lines {
        body.push('\n');
        body.push_str(l);
    }
    let mut out = String::new();
    let mut rest = body.as_str();
    loop {
        let next = [("{{ ref('", false), ("{{ source('", true)]
            .into_iter()
            .filter_map(|(open, source)| rest.find(open).map(|i| (i, open, source)))
            .min_by_key(|(i, _, _)| *i);
        let Some((i, open, source)) = next else { break };
        let after = &rest[i + open.len()..];
        let Some(j) = after.find("') }}") else {
            break;
        };
        out.push_str(&rest[..i]);
        // A source's arguments are `'<schema>', '<model>'`: the model is its last.
        let name = if source {
            after[..j].rsplit('\'').next().unwrap_or_default()
        } else {
            &after[..j]
        };
        out.push('"');
        out.push_str(name);
        out.push('"');
        rest = &after[j + "') }}".len()..];
    }
    out.push_str(rest);
    (out.trim_end().to_string(), first as u32 + 1)
}

/// SQL compared as the clause map and the parser write it: case and double quotes ignored, and
/// whitespace dropped, outside string literals.
pub fn norm(sql: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    for c in sql.chars() {
        if c == '\'' {
            quoted = !quoted;
            out.push(c);
        } else if quoted {
            out.push(c);
        } else if c != '"' && !c.is_whitespace() {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The top-level `AND` terms of a predicate (outside parentheses and string literals).
pub fn split_and(condition: &str) -> Vec<String> {
    let lower = condition.to_ascii_lowercase();
    let (bytes, lb) = (condition.as_bytes(), lower.as_bytes());
    let (mut depth, mut quoted, mut start, mut i) = (0i32, false, 0usize, 0usize);
    let mut out = vec![];
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => quoted = !quoted,
            b'(' if !quoted => depth += 1,
            b')' if !quoted => depth -= 1,
            _ if !quoted && depth == 0 && lb[i..].starts_with(b" and ") => {
                out.push(condition[start..i].trim().to_string());
                i += 5;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(condition[start..].trim().to_string());
    out.retain(|t| !t.is_empty());
    out
}

/// One `Filter` step per `AND`ed predicate, chained in the order written.
fn split_filters(g: TransformGraph) -> TransformGraph {
    // old index → (first, last) new index
    let mut range: Vec<(u32, u32)> = vec![];
    let mut nodes: Vec<TransformNode> = vec![];
    let mut inner: Vec<(NodeId, NodeId)> = vec![];
    for node in g.nodes {
        let first = nodes.len() as u32;
        match node {
            TransformNode::Filter { condition } if split_and(&condition).len() > 1 => {
                for (k, part) in split_and(&condition).into_iter().enumerate() {
                    if k > 0 {
                        let at = nodes.len() as u32;
                        inner.push((NodeId(at - 1), NodeId(at)));
                    }
                    nodes.push(TransformNode::Filter { condition: part });
                }
            }
            other => nodes.push(other),
        }
        range.push((first, nodes.len() as u32 - 1));
    }
    let last = |n: NodeId| NodeId(range[n.0 as usize].1);
    for n in &mut nodes {
        if let TransformNode::Join { left, right, .. } = n {
            *left = last(*left);
            *right = last(*right);
        }
    }
    let mut edges: Vec<(NodeId, NodeId)> = g
        .edges
        .iter()
        .map(|(a, b)| (last(*a), NodeId(range[b.0 as usize].0)))
        .collect();
    edges.extend(inner);
    edges.sort_by_key(|(a, b)| (a.0, b.0));
    TransformGraph {
        nodes,
        edges,
        origin: g.origin,
    }
}

/// The mart itself as the last step, fed by every step nothing else reads.
fn add_sink(mut g: TransformGraph, mart: &str, columns: Vec<String>) -> TransformGraph {
    let ends: Vec<u32> = (0..g.nodes.len() as u32)
        .filter(|i| !g.edges.iter().any(|(a, _)| a.0 == *i))
        .collect();
    let sink = NodeId(g.nodes.len() as u32);
    g.nodes.push(TransformNode::Sink {
        object_name: format!("gold.{mart}"),
        columns,
    });
    for e in ends {
        g.edges.push((NodeId(e), sink));
    }
    g
}

/// The terms a step was compiled from, or why it maps to none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mapping {
    Terms {
        terms: Vec<String>,
        rule: String,
        /// The silver table a `Source` step reads.
        table: Option<String>,
    },
    Unmapped(String),
}

fn terms(terms: Vec<String>, rule: String) -> Mapping {
    Mapping::Terms {
        terms,
        rule,
        table: None,
    }
}

struct Matcher<'a> {
    clauses: &'a [Clause],
    resolved: &'a [Resolved],
    grain: &'a [String],
    /// Prose filters not yet found in the SQL (their SQL is whatever was drafted).
    prose_filters: Vec<&'a Clause>,
}

impl<'a> Matcher<'a> {
    fn new(clauses: &'a [Clause], resolved: &'a [Resolved], grain: &'a [String]) -> Self {
        let prose_filters = clauses
            .iter()
            .filter(|c| c.kind == "filter" && c.sql.is_empty())
            .collect();
        Self {
            clauses,
            resolved,
            grain,
            prose_filters,
        }
    }

    fn find(&self, kind: &str, sql: &str) -> Option<&'a Clause> {
        let n = norm(sql);
        self.clauses
            .iter()
            .find(|c| c.kind == kind && !c.sql.is_empty() && norm(&c.sql) == n)
    }

    fn is_drafted(&self, term: &str) -> bool {
        self.resolved
            .iter()
            .any(|r| r.term == term && gold::fidelity(r) == "drafted")
    }

    /// A dimension or measure: by its expression and output name, or — when its term is drafted
    /// prose — by its output name alone (the expression is the model's).
    fn output(&self, kind: &str, output: &str, sql: &str) -> Option<&'a Clause> {
        let same_output = |c: &Clause| c.output.as_deref().is_some_and(|o| norm(o) == norm(output));
        let n = norm(sql);
        // Two terms may compile to one expression (`count(*)` twice): the output name decides.
        self.clauses
            .iter()
            .find(|c| c.kind == kind && same_output(c) && norm(&c.sql) == n)
            .or_else(|| {
                self.clauses.iter().find(|c| {
                    c.kind == kind && same_output(c) && c.terms.iter().all(|t| self.is_drafted(t))
                })
            })
    }

    fn how(&self, term: &str) -> String {
        self.resolved
            .iter()
            .find(|r| r.term == term)
            .map(|r| r.how.clone())
            .unwrap_or_default()
    }

    fn map(&mut self, node: &TransformNode) -> Mapping {
        match node {
            TransformNode::Source { object_name, .. } => self.source(object_name),
            TransformNode::Join { keys, .. } => self.join(keys),
            TransformNode::Filter { condition } => self.filter(condition),
            TransformNode::Aggregate { group_by, aggs } => self.aggregate(group_by, aggs),
            TransformNode::Calculate { output, expr } => self.calculate(output, expr),
            TransformNode::Sink { .. } => self.sink(),
            TransformNode::Unmapped { reason, .. } => {
                Mapping::Unmapped(format!("not understood by the parser: {reason}"))
            }
        }
    }

    fn source(&self, object_name: &str) -> Mapping {
        match self.find("source", object_name) {
            Some(c) if c.terms.is_empty() => {
                Mapping::Unmapped(format!("`{object_name}` is read, but no term needs it"))
            }
            Some(c) => Mapping::Terms {
                terms: c.terms.clone(),
                rule: format!(
                    "silver `{}`, read for {}",
                    c.table.as_deref().unwrap_or_default(),
                    list(&c.terms)
                ),
                table: c.table.clone(),
            },
            None => Mapping::Unmapped(format!(
                "`{object_name}` is read by no term of the requirement"
            )),
        }
    }

    fn join(&self, keys: &[(String, String)]) -> Mapping {
        let found: Vec<&Clause> = keys
            .iter()
            .map(|(a, b)| {
                self.find("join", &format!("{a} = {b}"))
                    .or_else(|| self.find("join", &format!("{b} = {a}")))
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default();
        match found.as_slice() {
            [c] if !c.terms.is_empty() => terms(
                c.terms.clone(),
                format!(
                    "declared foreign key {} (many-to-one: the base's rows are kept and never \
                     repeated)",
                    c.sql.replace('"', "`")
                ),
            ),
            _ => Mapping::Unmapped("a join along keys no term's join path declares".to_string()),
        }
    }

    fn filter(&mut self, condition: &str) -> Mapping {
        if let Some(c) = self.find("filter", condition) {
            return terms(c.terms.clone(), self.how(&c.terms[0]));
        }
        // A drafted predicate is found by elimination: one prose filter left takes it; several
        // are named together, since which predicate came from which draft is not recorded.
        if !self.prose_filters.is_empty() {
            let terms: Vec<String> = self
                .prose_filters
                .iter()
                .flat_map(|c| c.terms.clone())
                .collect();
            if self.prose_filters.len() == 1 {
                self.prose_filters.clear();
            }
            return Mapping::Terms {
                rule: "prose, drafted by a model".into(),
                terms,
                table: None,
            };
        }
        Mapping::Unmapped(format!("`{condition}` is no filter of the requirement"))
    }

    fn calculate(&self, output: &str, expr: &str) -> Mapping {
        match self
            .output("dimension", output, expr)
            .or_else(|| self.output("measure", output, expr))
        {
            Some(c) => terms(c.terms.clone(), self.how(&c.terms[0])),
            None => Mapping::Unmapped(format!(
                "`{output}` is computed by no term of the requirement"
            )),
        }
    }

    fn sink(&self) -> Mapping {
        let terms: Vec<String> = if self.grain.is_empty() {
            self.clauses
                .iter()
                .filter_map(|c| c.output.clone())
                .collect()
        } else {
            self.grain.to_vec()
        };
        Mapping::Terms {
            rule: format!("one row per {} (tested unique)", list(&terms)),
            terms,
            table: None,
        }
    }

    fn aggregate(&self, group_by: &[String], aggs: &[AggExpr]) -> Mapping {
        let mut terms = vec![];
        let mut missing = vec![];
        for g in group_by {
            match self.find("dimension", g) {
                Some(c) => terms.extend(c.terms.clone()),
                None => missing.push(g.clone()),
            }
        }
        for a in aggs {
            let call = format!("{}({})", a.func, a.arg);
            match self.output("measure", &a.output, &call) {
                Some(c) => terms.extend(c.terms.clone()),
                None => missing.push(format!("{call} as {}", a.output)),
            }
        }
        if !missing.is_empty() {
            return Mapping::Unmapped(format!(
                "{} maps to no term of the requirement",
                missing
                    .iter()
                    .map(|m| format!("`{m}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let rule = terms
            .iter()
            .map(|t| format!("{t}: {}", self.how(t)))
            .collect::<Vec<_>>()
            .join("; ");
        Mapping::Terms {
            terms,
            rule,
            table: None,
        }
    }
}

fn list(terms: &[String]) -> String {
    if terms.is_empty() {
        "nothing".into()
    } else {
        terms
            .iter()
            .map(|t| format!("`{t}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The parsed, split graph of a mart's SQL with the mart as its last step, and the plain SQL it
/// was parsed from (with its first line in the file).
pub fn mart_ir(m: &MartSql<'_>) -> anyhow::Result<(TransformGraph, String, u32)> {
    let (plain, start_line) = plain_sql(m.dbt_sql);
    anyhow::ensure!(!plain.is_empty(), "{}: the model has no SQL", m.path);
    let dialect = ekos_lakehouse_gold::parser(m.engine);
    let graphs = ekos_recovery::sql_transform_analyzer::parse_sql_to_statement_graphs(
        &dialect.preprocess(&plain),
        m.path,
        dialect.name(),
        &*dialect.sqlparser_dialect(),
    );
    let graph = graphs
        .into_iter()
        .map(|g| g.graph)
        .find(|g| !g.nodes.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{}: the parser found no query", m.path))?;
    let outputs: Vec<String> = m.clauses.iter().filter_map(|c| c.output.clone()).collect();
    let graph = add_sink(split_filters(graph), m.mart, outputs);
    Ok((graph, plain, start_line))
}

/// Every step's mapping, by node index.
pub fn mappings(m: &MartSql<'_>, graph: &TransformGraph) -> Vec<Mapping> {
    let mut matcher = Matcher::new(m.clauses, m.resolved, m.grain);
    graph.nodes.iter().map(|n| matcher.map(n)).collect()
}

fn node_index(o: &KirObject) -> Option<u32> {
    o.name.rsplit_once(':').and_then(|(_, i)| i.parse().ok())
}

/// The notes a step shows: those on the table a `Source` step reads, and the instruction an
/// instruction-made filter came from.
fn notes_for(m: &MartSql<'_>, map: &Mapping) -> Vec<Value> {
    let Mapping::Terms { terms, table, .. } = map else {
        return vec![];
    };
    let mut out: Vec<Value> = table
        .as_ref()
        .and_then(|t| m.notes_by_table.get(&t.to_ascii_lowercase()))
        .cloned()
        .unwrap_or_default();
    for t in terms {
        for n in m.notes_by_term.get(t).into_iter().flatten() {
            if !out.contains(n) {
                out.push(n.clone());
            }
        }
    }
    out
}

/// The chain of one mart (`ekos.transformation-graph/1`).
pub fn chain(m: &MartSql<'_>) -> anyhow::Result<ChainGraph> {
    chain_ir(m).map(|(g, _)| g)
}

/// [`chain`], with the IR graph it was drawn from (row counts are planned over it).
pub fn chain_ir(m: &MartSql<'_>) -> anyhow::Result<(ChainGraph, TransformGraph)> {
    let (graph, plain, start_line) = mart_ir(m)?;
    let maps = mappings(m, &graph);
    let kir = lower_to_kir(&graph);
    let objects: Vec<KirObject> = kir.objects.clone();
    let first_id = transform_node_kir_id(&graph.origin, 0);
    let first = objects
        .iter()
        .find(|o| o.id == first_id)
        .ok_or_else(|| anyhow::anyhow!("the mart's graph has no first step"))?;
    let selection = transform_chain::select(&objects, first).map_err(|e| anyhow::anyhow!(e))?;
    let evidence: HashMap<KirId, Vec<EvidenceView>> = objects
        .iter()
        .map(|o| {
            let views = kir
                .evidence
                .iter()
                .filter(|ev| o.evidence.contains(&ev.id))
                .map(|ev| EvidenceView {
                    path: m.path.to_string(),
                    line: ev.location.line,
                    fragment: ev.fragment.clone(),
                    confidence: ev.confidence,
                })
                .collect();
            (o.id, views)
        })
        .collect();
    let annotations: HashMap<String, Vec<Value>> = objects
        .iter()
        .filter_map(|o| {
            let notes = notes_for(m, maps.get(node_index(o)? as usize)?);
            (!notes.is_empty()).then(|| (o.id.0.to_string(), notes))
        })
        .collect();
    let ctx = Context {
        edges: kir.relationships.iter().map(|r| (r.from, r.to)).collect(),
        evidence,
        annotations,
        source: Some(SourceText {
            path: m.path.to_string(),
            start_line,
            text: plain,
        }),
        expand: Expand::All,
    };
    let mut g = transform_chain::build(&selection, &ctx);
    g.transformation.kind = "mart".into();
    g.transformation.name = m.mart.to_string();
    g.transformation.key = format!("{}@{}", m.path, m.version);
    g.transformation.recovered = false;
    for n in &mut g.nodes {
        let Some(map) = n.index.and_then(|i| maps.get(i as usize)) else {
            continue;
        };
        match map {
            Mapping::Terms { terms, rule, .. } => {
                n.term = Some(terms.join(", "));
                n.rule = Some(rule.clone());
                let drafted = terms.iter().any(|t| {
                    m.resolved
                        .iter()
                        .any(|r| &r.term == t && gold::fidelity(r) == "drafted")
                });
                n.fidelity = Some(if drafted { "drafted" } else { "exact" }.into());
                n.status = None;
            }
            Mapping::Unmapped(why) => {
                n.term = None;
                n.rule = None;
                n.fidelity = None;
                n.status = Some("unmapped".into());
                n.summary = format!("{} — {why}", n.summary);
            }
        }
    }
    let unmapped = g
        .nodes
        .iter()
        .filter(|n| n.status.as_deref() == Some("unmapped"))
        .count();
    g.counts.unmapped = unmapped;
    g.counts.mapped = g.counts.nodes - unmapped;
    g.rows.reason = "row counts per step: `lakehouse marts graph --rows` (RFC 0257 §3.3)".into();
    Ok((g, graph))
}

/// The counts of §3.3 on each step (`rows`), and their short form in the step's label.
pub fn with_rows(v: &mut Value, rows: &BTreeMap<u32, Value>, note: &str) {
    if let Some(nodes) = v["nodes"].as_array_mut() {
        for n in nodes {
            let Some(r) = n["index"].as_u64().and_then(|i| rows.get(&(i as u32))) else {
                continue;
            };
            let show = |x: &Value| match x {
                Value::String(s) => s.clone(),
                Value::Null => "?".into(),
                other => other.to_string(),
            };
            if r.get("withheld").is_none() {
                let label = format!(
                    "{} · {}",
                    n["label"].as_str().unwrap_or_default(),
                    show(&r["out"])
                );
                n["label"] = json!(label);
                n["rows_out"] = json!(show(&r["out"]));
                n["rows_in"] = json!(
                    r["in"]
                        .as_array()
                        .map(|a| a.iter().map(show).collect::<Vec<_>>().join(" + "))
                        .unwrap_or_default()
                );
            } else {
                n["rows_out"] = r["withheld"].clone();
            }
            n["rows"] = r.clone();
        }
    }
    v["rows"] = json!({"available": true, "reason": note});
}

/// [`chain`] as JSON, each node with a `badge` (its fidelity, else its status) for the console's
/// graph view, and `defects` (the steps no term produced) at the top.
pub fn to_json(g: &ChainGraph) -> Value {
    let mut v = serde_json::to_value(g).unwrap_or(Value::Null);
    if let Some(nodes) = v["nodes"].as_array_mut() {
        for n in nodes {
            let badge = n["fidelity"]
                .as_str()
                .or(n["status"].as_str())
                .unwrap_or_default()
                .to_string();
            n["badge"] = json!(badge);
            let notes: Vec<String> = n["annotations"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| {
                    format!(
                        "{} ({}, {}): {}",
                        a["author"].as_str().unwrap_or_default(),
                        a["kind"].as_str().unwrap_or_default(),
                        a["state"].as_str().unwrap_or_default(),
                        a["text"].as_str().unwrap_or_default()
                    )
                })
                .collect();
            n["notes"] = json!(notes.join(" · "));
        }
    }
    v["defects"] = json!(g.counts.unmapped);
    v
}

/// The chain as text, one step per line.
pub fn render(g: &ChainGraph) -> String {
    let mut s = format!(
        "mart `{}` ({}) — {} step(s), {} mapped to no term\n",
        g.transformation.name, g.transformation.key, g.counts.nodes, g.counts.unmapped
    );
    for n in &g.nodes {
        let index = n.index.map_or_else(|| "-".to_string(), |i| i.to_string());
        let badge = n
            .fidelity
            .as_deref()
            .or(n.status.as_deref())
            .unwrap_or_default();
        s.push_str(&format!(
            "  [{index:>2}] {:<9} {:<9} {}\n",
            n.node_type, badge, n.summary
        ));
        if let Some(t) = &n.term {
            s.push_str(&format!("       term: {t}\n"));
        }
        if !n.annotations.is_empty() {
            s.push_str(&format!("       {} note(s)\n", n.annotations.len()));
        }
    }
    s
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const SALES: &str = "-- Generated by ekos-lakehouse (RFC 0195) from marts/sales_by_month.md. Approve by merging; do not edit.
{{ config(materialized='table', schema='gold', alias='sales_by_month') }}

select
    date_trunc('month', \"C_ORDER\".\"DATEORDERED\") as \"month\",
    \"C_BPARTNER\".\"NAME\" as \"customer\",
    sum(\"C_ORDER\".\"GRANDTOTAL\") as \"total\",
    count(*) as \"orders\"
from {{ ref('ADEMPIERE__C_ORDER') }} as \"C_ORDER\"
left join {{ ref('ADEMPIERE__C_BPARTNER') }} as \"C_BPARTNER\"
    on \"C_ORDER\".\"C_BPARTNER_ID\" = \"C_BPARTNER\".\"C_BPARTNER_ID\"
where \"C_ORDER\".\"ISSOTRX\" = true
    and \"C_ORDER\".\"DOCSTATUS\" = 'CO'
group by date_trunc('month', \"C_ORDER\".\"DATEORDERED\"), \"C_BPARTNER\".\"NAME\"
";

    fn clause(kind: &'static str, terms: &[&str], sql: &str, output: Option<&str>) -> Clause {
        Clause {
            kind,
            terms: terms.iter().map(|t| t.to_string()).collect(),
            sql: sql.into(),
            output: output.map(str::to_string),
            table: None,
        }
    }

    fn sales_clauses() -> Vec<Clause> {
        vec![
            clause(
                "source",
                &["total", "orders", "month"],
                "\"ADEMPIERE__C_ORDER\"",
                None,
            ),
            clause("source", &["customer"], "\"ADEMPIERE__C_BPARTNER\"", None),
            clause(
                "join",
                &["customer"],
                "\"C_ORDER\".\"C_BPARTNER_ID\" = \"C_BPARTNER\".\"C_BPARTNER_ID\"",
                None,
            ),
            clause(
                "filter",
                &["c_order.issotrx is true"],
                "\"C_ORDER\".\"ISSOTRX\" = true",
                None,
            ),
            clause(
                "filter",
                &["c_order.docstatus = 'CO'"],
                "\"C_ORDER\".\"DOCSTATUS\" = 'CO'",
                None,
            ),
            clause(
                "dimension",
                &["month"],
                "date_trunc('month', \"C_ORDER\".\"DATEORDERED\")",
                Some("month"),
            ),
            clause(
                "dimension",
                &["customer"],
                "\"C_BPARTNER\".\"NAME\"",
                Some("customer"),
            ),
            clause(
                "measure",
                &["total"],
                "sum(\"C_ORDER\".\"GRANDTOTAL\")",
                Some("total"),
            ),
            clause("measure", &["orders"], "count(*)", Some("orders")),
        ]
    }

    fn input<'a>(sql: &'a str, clauses: &'a [Clause], grain: &'a [String]) -> MartSql<'a> {
        MartSql {
            mart: "sales_by_month",
            path: "dbt/models/gold/gold__sales_by_month.sql",
            dbt_sql: sql,
            version: "committed",
            engine: Default::default(),
            grain,
            resolved: &[],
            clauses,
            notes_by_table: BTreeMap::new(),
            notes_by_term: BTreeMap::new(),
        }
    }

    #[test]
    fn refs_become_model_names_and_the_header_is_dropped() {
        let (plain, line) = plain_sql(SALES);
        assert_eq!(line, 4);
        assert!(plain.starts_with("select\n"), "{plain}");
        assert!(plain.contains("from \"ADEMPIERE__C_ORDER\" as \"C_ORDER\""));
        assert!(!plain.contains("{{"));
    }

    #[test]
    fn and_is_split_outside_parentheses_and_literals() {
        assert_eq!(
            split_and("a = 1 AND (b = 2 and c = 3) AND d = 'x and y'"),
            ["a = 1", "(b = 2 and c = 3)", "d = 'x and y'"]
        );
        assert_eq!(norm("\"C_ORDER\".\"X\" = 'Co Ab'"), "c_order.x='Co Ab'");
    }

    /// RFC 0257 §4 phase 1: every step of a generated mart maps to a term.
    #[test]
    fn every_step_of_a_generated_mart_maps_to_a_term() {
        let clauses = sales_clauses();
        let grain = vec!["month".to_string(), "customer".to_string()];
        let g = chain(&input(SALES, &clauses, &grain)).unwrap();
        assert_eq!(g.format, "ekos.transformation-graph/1");
        assert!(!g.transformation.recovered);
        assert_eq!(g.transformation.kind, "mart");
        let types: Vec<&str> = g.nodes.iter().map(|n| n.node_type.as_str()).collect();
        assert_eq!(
            types,
            [
                "Source",
                "Source",
                "Join",
                "Filter",
                "Filter",
                "Aggregate",
                "Calculate",
                "Sink"
            ],
            "two ANDed predicates are two steps; the mart is the last"
        );
        for n in &g.nodes {
            assert!(n.status.is_none(), "{} is unmapped: {}", n.label, n.summary);
            assert!(
                n.term.is_some() && n.fidelity.as_deref() == Some("exact"),
                "{n:?}"
            );
        }
        assert_eq!(g.nodes[4].term.as_deref(), Some("c_order.docstatus = 'CO'"));
        assert_eq!(
            g.nodes[5].term.as_deref(),
            Some("month, customer, total, orders")
        );
        assert_eq!(g.nodes[7].term.as_deref(), Some("month, customer"));
        assert_eq!(g.counts.unmapped, 0);
        assert!(g.nodes[4].span.is_some(), "{:?}", g.nodes[4]);
        // Two measures that compile to the same expression map by their output names.
        let mut twice = clauses.clone();
        twice.push(clause("measure", &["lines"], "count(*)", Some("lines")));
        let sql = SALES.replace(
            "count(*) as \"orders\"",
            "count(*) as \"orders\",\n    count(*) as \"lines\"",
        );
        let g2 = chain(&input(&sql, &twice, &grain)).unwrap();
        assert_eq!(g2.counts.unmapped, 0, "{:#?}", g2.nodes);
        // Edges stay a chain: join → filter → filter → aggregate.
        let id = |i: usize| g.nodes[i].id.clone();
        assert!(g.edges.iter().any(|e| e.from == id(2) && e.to == id(3)));
        assert!(g.edges.iter().any(|e| e.from == id(3) && e.to == id(4)));
        assert!(g.edges.iter().any(|e| e.from == id(4) && e.to == id(5)));
        let v = to_json(&g);
        assert_eq!(v["nodes"][0]["badge"], "exact");
        assert_eq!(v["defects"], 0);
    }

    /// RFC 0257 §4 phase 1: a mart with a planted unmapped step is flagged.
    #[test]
    fn a_planted_step_no_term_produced_is_flagged() {
        let clauses = sales_clauses();
        let grain = vec!["month".to_string(), "customer".to_string()];
        // A hand edit: a predicate no filter term states.
        let edited = SALES.replace(
            "and \"C_ORDER\".\"DOCSTATUS\" = 'CO'",
            "and \"C_ORDER\".\"DOCSTATUS\" = 'CO'\n    and \"C_ORDER\".\"ISACTIVE\" = true",
        );
        let g = chain(&input(&edited, &clauses, &grain)).unwrap();
        let bad: Vec<&str> = g
            .nodes
            .iter()
            .filter(|n| n.status.as_deref() == Some("unmapped"))
            .map(|n| n.summary.as_str())
            .collect();
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert!(
            bad[0].contains("ISACTIVE") && bad[0].contains("no filter of the requirement"),
            "{bad:?}"
        );
        assert_eq!(g.counts.unmapped, 1);
        let v = to_json(&g);
        assert_eq!(v["defects"], 1);
        assert!(
            v["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["badge"] == "unmapped")
        );
    }

    /// EKOS RFC 0259 L1: a DuckDB mart reads silver as sources named after the models; parsed in
    /// DuckDB's dialect, its chain is the Trino mart's chain, every step mapped.
    #[test]
    fn a_duckdb_mart_reads_and_maps_as_the_trino_one() {
        let duck = SALES
            .replace(
                "{{ ref('ADEMPIERE__C_ORDER') }}",
                "{{ source('silver_adempiere', 'ADEMPIERE__C_ORDER') }}",
            )
            .replace(
                "{{ ref('ADEMPIERE__C_BPARTNER') }}",
                "{{ source('silver_adempiere', 'ADEMPIERE__C_BPARTNER') }}",
            );
        assert_eq!(plain_sql(&duck), plain_sql(SALES));
        let clauses = sales_clauses();
        let grain = vec!["month".to_string(), "customer".to_string()];
        let mut m = input(&duck, &clauses, &grain);
        m.engine = ekos_lakehouse_gold::GoldEngine::Duckdb;
        let g = chain(&m).unwrap();
        let t = chain(&input(SALES, &clauses, &grain)).unwrap();
        assert_eq!(g.counts.unmapped, 0, "{:#?}", g.nodes);
        let types = |g: &ChainGraph| {
            g.nodes
                .iter()
                .map(|n| (n.node_type.clone(), n.term.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(types(&g), types(&t));
    }
}

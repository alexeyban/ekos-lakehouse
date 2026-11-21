//! A Talend job's data components as nodes of the shared relational IR (EKOS RFC 0267
//! §Components). Pure: the job (joblets already inlined) and a catalogue in, an [`Ir`] and what
//! was left out of the data flow out.

use std::collections::{BTreeMap, HashMap};

use ekos_lakehouse_etl_expr::{Expr, Fidelity, Gap, KType, Lowered};
use ekos_lakehouse_etl_rel::{
    Agg, Catalog, Field, Ir, JoinKind, LookupValue, NodeId, Op, RelNode, Sink, SinkMode, SortKey,
    SourceQuery, TableRef,
};
use ekos_talend::{Category, Component, Job, Link, LinkKind, Mapper, MapperOutput, Param};

use crate::expr::{ColumnLookup, Scope, lower, talend_type};

/// Components that hand their input on unchanged.
const PASS_THROUGH: &[&str] = &[
    "tReplicate",
    "tLogRow",
    "tFlowMeter",
    "tWarn",
    "tSchemaComplianceCheck",
    "tHashOutput",
    "tBufferOutput",
    "tConvertType",
    "joblet_input",
    "joblet_output",
];

/// One job's lowering.
#[derive(Debug, Default)]
pub struct Lowering {
    pub ir: Ir,
    /// Components outside the data flow (orchestration, logging, connections), with why.
    pub skipped: Vec<(String, String)>,
    /// Expressions lowered and expressions that stopped at a gap, by the gap's reason.
    pub expressions: usize,
    pub expression_gaps: BTreeMap<String, usize>,
}

/// Lowers one job (expand joblets first: `Project::expand`).
pub fn lower_job(job: &Job, catalog: &dyn Catalog) -> Lowering {
    let mut b = Builder {
        job,
        catalog,
        out: Lowering {
            ir: Ir {
                transformation: job.path.clone(),
                nodes: Vec::new(),
            },
            ..Lowering::default()
        },
        ports: HashMap::new(),
    };
    let data: Vec<&Component> = job
        .components
        .iter()
        .filter(|c| c.category.is_data() || PASS_THROUGH.contains(&c.component_type.as_str()))
        .collect();
    for c in &job.components {
        if !data.iter().any(|d| d.unique_name == c.unique_name) {
            b.out.skipped.push((
                c.unique_name.clone(),
                format!(
                    "{} ({}): not part of the data flow",
                    c.component_type,
                    c.category.as_str()
                ),
            ));
        }
    }
    // Hop order: a component once every component feeding it is done; a tHashInput after every
    // tHashOutput.
    let mut done: Vec<&str> = Vec::new();
    let mut pending: Vec<&Component> = data.clone();
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|c| {
            let ready = inbound(job, c).iter().all(|l| {
                done.contains(&l.from.as_str()) || !data.iter().any(|d| d.unique_name == l.from)
            }) && (!is_hash_input(c)
                || data
                    .iter()
                    .filter(|d| is_hash_output(d))
                    .all(|d| done.contains(&d.unique_name.as_str())));
            if ready {
                b.component(c);
                done.push(c.unique_name.as_str());
            }
            !ready
        });
        if pending.len() == before {
            for c in &pending {
                b.gap(c, "", Vec::new(), "on a cycle of row links".into());
            }
            break;
        }
    }
    b.out
}

fn inbound<'a>(job: &'a Job, c: &Component) -> Vec<&'a Link> {
    job.links
        .iter()
        .filter(|l| l.to == c.unique_name && l.kind != LinkKind::Iterate)
        .collect()
}

fn is_hash_input(c: &Component) -> bool {
    matches!(c.component_type.as_str(), "tHashInput" | "tBufferInput")
}

fn is_hash_output(c: &Component) -> bool {
    matches!(c.component_type.as_str(), "tHashOutput" | "tBufferOutput")
}

fn param<'a>(c: &'a Component, name: &str) -> Option<&'a str> {
    c.params
        .get(name)
        .and_then(Param::text)
        .filter(|v| !v.is_empty())
}

fn rows<'a>(c: &'a Component, name: &str) -> &'a [BTreeMap<String, String>] {
    c.params.get(name).map(Param::rows).unwrap_or_default()
}

/// A component's output schema: the one named `name` (a tMap output), else its `FLOW`, else its
/// first.
fn schema(c: &Component, name: &str) -> Vec<Field> {
    let s = c
        .schemas
        .iter()
        .find(|s| !name.is_empty() && s.name == name)
        .or_else(|| c.schemas.iter().find(|s| s.connector == "FLOW"))
        .or_else(|| c.schemas.first());
    s.map(|s| {
        s.columns
            .iter()
            .map(|col| {
                let mut f = Field::new(&col.name, talend_type(col.talend_type.as_deref()));
                f.length = col.length.map_or(-1, |l| l as i32);
                f.precision = col.precision.map_or(-1, |p| p as i32);
                f
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The row number a tUniqRow's duplicates output is computed from.
const DUP_ROW: &str = "__talend_row_in_key";

fn keys_for_window(c: &Component) -> Vec<String> {
    rows(c, "UNIQUE_KEY")
        .iter()
        .filter(|r| r.get("KEY_ATTRIBUTE").map(String::as_str) == Some("true"))
        .filter_map(|r| r.get("SCHEMA_COLUMN").cloned())
        .collect()
}

/// The name a column of input `q` carries inside a tMap's combined row.
fn qualified(q: &str, col: &str) -> String {
    format!("{q}__{col}")
}

struct Builder<'a> {
    job: &'a Job,
    catalog: &'a dyn Catalog,
    out: Lowering,
    /// `(component, port)` → the node rows leave from. Port `""` is the main output; a tMap's
    /// outputs are their names; `REJECT` / `DUPLICATE` the second outputs.
    ports: HashMap<(String, String), NodeId>,
}

impl Builder<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        c: &Component,
        op: Op,
        inputs: Vec<NodeId>,
        schema: Vec<Field>,
        fidelity: Fidelity,
        notes: Vec<String>,
        branch: Option<String>,
    ) -> NodeId {
        let id = self.out.ir.nodes.len();
        self.out.ir.nodes.push(RelNode {
            id,
            step: c.unique_name.clone(),
            step_type: c.component_type.clone(),
            branch,
            op,
            inputs,
            schema,
            fidelity,
            notes,
            line: c.line,
        });
        id
    }

    fn gap(&mut self, c: &Component, port: &str, inputs: Vec<NodeId>, reason: String) -> NodeId {
        let schema = schema(c, port);
        let id = self.push(
            c,
            Op::Gap { reason },
            inputs,
            schema,
            Fidelity::Gap,
            Vec::new(),
            (!port.is_empty()).then(|| port.to_string()),
        );
        self.ports
            .insert((c.unique_name.clone(), port.to_string()), id);
        id
    }

    fn set(&mut self, c: &Component, port: &str, id: NodeId) {
        self.ports
            .insert((c.unique_name.clone(), port.to_string()), id);
    }

    /// The node a link brings rows from.
    fn upstream(&self, l: &Link) -> Option<NodeId> {
        let from = self.job.component(&l.from)?;
        let port = if from.mapper.is_some() {
            l.metaname.clone().unwrap_or_default()
        } else {
            match l.kind {
                LinkKind::Reject => "REJECT".to_string(),
                LinkKind::Duplicate => "DUPLICATE".to_string(),
                _ => String::new(),
            }
        };
        self.ports
            .get(&(l.from.clone(), port))
            .or_else(|| self.ports.get(&(l.from.clone(), String::new())))
            .copied()
    }

    fn main_input(&self, c: &Component) -> Option<NodeId> {
        let links = inbound(self.job, c);
        links
            .iter()
            .find(|l| l.kind == LinkKind::Main)
            .or_else(|| links.first())
            .and_then(|l| self.upstream(l))
    }

    fn input_schema(&self, id: NodeId) -> Vec<Field> {
        self.out.ir.nodes[id].schema.clone()
    }

    fn count_unit(&mut self, r: Result<(), Gap>) {
        match r {
            Ok(()) => self.out.expressions += 1,
            Err(g) => {
                let key: String = g.reason.chars().take(80).collect();
                *self.out.expression_gaps.entry(key).or_default() += 1;
            }
        }
    }

    fn count(&mut self, r: &Result<(Lowered, KType), Gap>) {
        match r {
            Ok(_) => self.out.expressions += 1,
            Err(g) => {
                let key: String = g.reason.chars().take(80).collect();
                *self.out.expression_gaps.entry(key).or_default() += 1;
            }
        }
    }

    fn component(&mut self, c: &Component) {
        // A component nothing feeds that does not produce rows itself is disconnected: not part
        // of any flow, so not a translation gap.
        let source = c.category == Category::Input
            || matches!(c.logical.as_str(), "db_input" | "elt_input")
            || matches!(
                c.component_type.as_str(),
                "tRowGenerator" | "tFixedFlowInput"
            )
            || is_hash_input(c);
        // A SQL statement component (`tDBRow`, `tSnowflakeRow`, ELT) runs from a trigger and changes
        // data without a row link: never skipped.
        let statement = c.category == Category::Elt;
        if !source
            && !statement
            && c.mapper.is_none()
            && c.elt.is_none()
            && inbound(self.job, c).is_empty()
        {
            self.out.skipped.push((
                c.unique_name.clone(),
                format!("{}: no row link in, not part of any flow", c.component_type),
            ));
            return;
        }
        if let Some(m) = &c.mapper {
            return self.tmap(c, m);
        }
        let t = c.component_type.as_str();
        if PASS_THROUGH.contains(&t) {
            return match self.main_input(c) {
                Some(i) => self.set(c, "", i),
                None => {
                    self.gap(c, "", Vec::new(), format!("{t} with no row link in"));
                }
            };
        }
        if is_hash_input(c) {
            return self.hash_input(c);
        }
        match c.logical.as_str() {
            "db_input" => self.db_input(c),
            "elt_input" => self.elt_input(c),
            l if l.starts_with("db_output") || l == "elt_output" => self.sink(c),
            _ => match t {
                "tFilterRow" => self.filter_row(c),
                "tRowGenerator" => self.row_generator(c, "VALUES", "ARRAY"),
                "tFixedFlowInput" => self.row_generator(c, "VALUES", "VALUE"),
                "tJoin" => self.join(c),
                "tUniqRow" => self.uniq_row(c),
                "tAggregateRow" | "tAggregateSortedRow" => self.aggregate(c),
                "tSortRow" => self.sort(c),
                "tFilterColumns" => self.filter_columns(c),
                "tUnite" => self.unite(c),
                _ if c.category == Category::Input && t.starts_with("tFileInput") => {
                    self.file_input(c)
                }
                _ if c.category == Category::Output && t.starts_with("tFileOutput") => {
                    self.file_output(c)
                }
                _ => {
                    let inputs: Vec<NodeId> = inbound(self.job, c)
                        .iter()
                        .filter_map(|l| self.upstream(l))
                        .collect();
                    self.gap(
                        c,
                        "",
                        inputs,
                        format!(
                            "{t} ({}) has no relational lowering yet",
                            c.category.as_str()
                        ),
                    );
                }
            },
        }
    }

    fn table_ref(&self, c: &Component, table_param: &str, schema_param: &str) -> Option<TableRef> {
        let table = self.job.fold(param(c, table_param)?).text;
        let schema = param(c, schema_param)
            .or_else(|| {
                param(c, "CONNECTION")
                    .and_then(|n| self.job.component(n))
                    .and_then(|conn| param(conn, "SCHEMA_DB"))
            })
            .map(|s| self.job.fold(s))
            .filter(|f| f.complete() && !f.text.is_empty())
            .map(|f| f.text);
        (!table.is_empty()).then_some(TableRef {
            connection: None,
            schema,
            table,
        })
    }

    fn db_input(&mut self, c: &Component) {
        let fields = schema(c, "");
        let table = self.table_ref(c, "TABLE", "SCHEMA_DB");
        let (text, mut fidelity, mut notes) = match param(c, "QUERY") {
            Some(q) => {
                let f = self.job.fold(q);
                if f.complete() {
                    (f.text, Fidelity::Exact, Vec::new())
                } else {
                    (
                        f.text,
                        Fidelity::Rewritten,
                        vec![format!(
                            "the query depends on values known only at run time: {}",
                            f.holes.join(", ")
                        )],
                    )
                }
            }
            None => match &table {
                Some(t) => (
                    format!(
                        "SELECT {} FROM {}",
                        fields
                            .iter()
                            .map(|f| f.name.clone())
                            .collect::<Vec<_>>()
                            .join(", "),
                        t.table
                    ),
                    Fidelity::Exact,
                    Vec::new(),
                ),
                None => {
                    self.gap(
                        c,
                        "",
                        Vec::new(),
                        "database input without a query or table".into(),
                    );
                    return;
                }
            },
        };
        if let Some(t) = &table
            && let Some(known) = self.catalog.columns(t)
            && known.len() != fields.len()
        {
            fidelity = fidelity.max(Fidelity::Rewritten);
            notes.push(format!(
                "the component's schema has {} column(s); the catalogue's table has {}",
                fields.len(),
                known.len()
            ));
        }
        let op = Op::Source {
            query: SourceQuery::Sql {
                text,
                tables: table.into_iter().collect(),
                limit: None,
            },
        };
        let id = self.push(c, op, Vec::new(), fields, fidelity, notes, None);
        self.set(c, "", id);
    }

    fn elt_input(&mut self, c: &Component) {
        let fields = schema(c, "");
        let Some(t) = self.table_ref(c, "ELT_TABLE_NAME", "ELT_SCHEMA_NAME") else {
            self.gap(c, "", Vec::new(), "ELT input without a table".into());
            return;
        };
        let name = match &t.schema {
            Some(s) => format!("{s}.{}", t.table),
            None => t.table.clone(),
        };
        let text = format!(
            "SELECT {} FROM {name}",
            fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let op = Op::Source {
            query: SourceQuery::Sql {
                text,
                tables: vec![t],
                limit: None,
            },
        };
        let id = self.push(c, op, Vec::new(), fields, Fidelity::Exact, Vec::new(), None);
        self.set(c, "", id);
    }

    fn file_input(&mut self, c: &Component) {
        let fields = schema(c, "");
        let file = param(c, "FILENAME")
            .map(|f| self.job.fold(f).text)
            .unwrap_or_default();
        let base = file.rsplit(['/', '\\']).next().unwrap_or(&file).to_string();
        let t = TableRef {
            connection: None,
            schema: Some("files".into()),
            table: base.clone(),
        };
        let op = Op::Source {
            query: SourceQuery::Sql {
                text: format!(
                    "SELECT {} FROM files.\"{base}\"",
                    fields
                        .iter()
                        .map(|f| f.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                tables: vec![t],
                limit: None,
            },
        };
        let note = format!(
            "{} reads the file `{file}`; the translation reads it as a landed table (RFC 0268 decides seed or source)",
            c.component_type
        );
        let id = self.push(
            c,
            op,
            Vec::new(),
            fields,
            Fidelity::Rewritten,
            vec![note],
            None,
        );
        self.set(c, "", id);
    }

    fn hash_input(&mut self, c: &Component) {
        let wanted = (param(c, "LINK_WITH") == Some("true"))
            .then(|| param(c, "LIST"))
            .flatten();
        let from = self
            .job
            .components
            .iter()
            .filter(|d| is_hash_output(d))
            .find(|d| wanted.is_none_or(|w| d.unique_name == w))
            .and_then(|d| {
                self.ports
                    .get(&(d.unique_name.clone(), String::new()))
                    .copied()
            });
        match from {
            Some(id) => self.set(c, "", id),
            None => {
                self.gap(
                    c,
                    "",
                    Vec::new(),
                    "no tHashOutput in the job feeds it".into(),
                );
            }
        }
    }

    /// Java text of tFilterRow's condition over `input_row`: the simple condition table (joined
    /// by `LOGICAL_OP`) and, in advanced mode, the advanced condition.
    fn filter_text(&self, c: &Component) -> Result<Option<String>, Gap> {
        let join = if param(c, "LOGICAL_OP") == Some("||") {
            " || "
        } else {
            " && "
        };
        let mut parts = Vec::new();
        for r in rows(c, "CONDITIONS") {
            let col = r
                .get("INPUT_COLUMN")
                .map(String::as_str)
                .unwrap_or_default();
            let op = r.get("OPERATOR").map(String::as_str).unwrap_or("==");
            let value = r.get("RVALUE").map(String::as_str).unwrap_or_default();
            let f = r.get("FUNCTION").map(String::as_str).unwrap_or_default();
            let source = format!("input_row.{col}");
            let text = if f.is_empty() {
                format!("{source} {op} {value}")
            } else if f.contains("$source") {
                f.replace("$source", &source)
                    .replace("$target", value)
                    .replace("$operator", op)
            } else {
                return Err(Gap::new(format!("tFilterRow function `{f}`")));
            };
            parts.push(format!("({text})"));
        }
        if param(c, "USE_ADVANCED") == Some("true")
            && let Some(adv) = param(c, "ADVANCED_COND")
        {
            let code: String = adv
                .lines()
                .map(|l| l.split("//").next().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" ");
            if !code.trim().is_empty() {
                parts.push(format!("({})", code.trim()));
            }
        }
        Ok((!parts.is_empty()).then(|| parts.join(join)))
    }

    /// The columns of `node`, named `<qualifier>.<column>`.
    fn columns_of(&self, node: NodeId, qualifier: &str) -> Box<ColumnLookup<'static>> {
        let fields = self.input_schema(node);
        let qualifier = qualifier.to_string();
        Box::new(move |q: &str, col: &str| {
            (q == qualifier)
                .then(|| fields.iter().find(|f| f.name == col))
                .flatten()
                .map(|f| (Expr::col(&f.name), f.ty))
        })
    }

    fn filter_row(&mut self, c: &Component) {
        let Some(input) = self.main_input(c) else {
            self.gap(c, "", Vec::new(), "tFilterRow with no row link in".into());
            return;
        };
        let lowered = match self.filter_text(c) {
            Ok(None) => {
                self.set(c, "", input);
                return;
            }
            Ok(Some(text)) => {
                let column = self.columns_of(input, "input_row");
                let context = context_of(self.job);
                let global = global_of(self.job);
                let none = |_: &str| None;
                lower(
                    &text,
                    &Scope {
                        column: &column,
                        var: &none,
                        context: &context,
                        job: &self.job.name,
                        global: &global,
                    },
                )
            }
            Err(g) => Err(g),
        };
        self.count(&lowered);
        let schema = self.input_schema(input);
        match lowered {
            Ok((l, _)) => {
                let pred = Expr::two_valued(l.expr.clone());
                let keep = self.push(
                    c,
                    Op::Filter {
                        predicate: pred.clone(),
                    },
                    vec![input],
                    schema.clone(),
                    l.fidelity,
                    l.notes.clone(),
                    None,
                );
                self.set(c, "", keep);
                let mut notes = l.notes.clone();
                notes.push("the rejected rows are those the condition does not keep".into());
                let reject = self.push(
                    c,
                    Op::Filter {
                        predicate: Expr::Unary {
                            op: ekos_lakehouse_etl_expr::UnOp::Not,
                            arg: Box::new(pred),
                        },
                    },
                    vec![input],
                    schema,
                    l.fidelity.max(Fidelity::Rewritten),
                    notes,
                    Some("REJECT".into()),
                );
                self.set(c, "REJECT", reject);
            }
            Err(g) => {
                self.gap(c, "", vec![input], g.reason.clone());
                self.gap(c, "REJECT", vec![input], g.reason);
            }
        }
    }

    fn uniq_row(&mut self, c: &Component) {
        let Some(input) = self.main_input(c) else {
            self.gap(c, "", Vec::new(), "tUniqRow with no row link in".into());
            return;
        };
        let keys: Vec<String> = rows(c, "UNIQUE_KEY")
            .iter()
            .filter(|r| r.get("KEY_ATTRIBUTE").map(String::as_str) == Some("true"))
            .filter_map(|r| r.get("SCHEMA_COLUMN").cloned())
            .collect();
        let schema = self.input_schema(input);
        let id = self.push(
            c,
            Op::Distinct { keys, count: None },
            vec![input],
            schema,
            Fidelity::Rewritten,
            vec!["Talend keeps the first row of each key by arrival order; the SQL keeps one row per key".into()],
            None,
        );
        self.set(c, "", id);
        // The duplicates: every row of a key but the first, by arrival order.
        let mut windowed = self.input_schema(input);
        windowed.push(Field::new(DUP_ROW, KType::Integer));
        let w = self.push(
            c,
            Op::Window {
                partition: keys_for_window(c),
                order: Vec::new(),
                items: vec![(
                    DUP_ROW.to_string(),
                    ekos_lakehouse_etl_rel::WindowFn::Sequence { start: 1, increment: 1 },
                )],
            },
            vec![input],
            windowed.clone(),
            Fidelity::Rewritten,
            vec!["rows are numbered per key in arrival order, which SQL does not keep: the order must be stated".into()],
            Some("DUPLICATE".into()),
        );
        let schema = self.input_schema(input);
        let dup = self.push(
            c,
            Op::Filter {
                predicate: Expr::bin(
                    ekos_lakehouse_etl_expr::BinOp::Gt,
                    Expr::col(DUP_ROW),
                    Expr::int(1),
                ),
            },
            vec![w],
            windowed,
            Fidelity::Rewritten,
            Vec::new(),
            Some("DUPLICATE".into()),
        );
        let items = schema
            .iter()
            .map(|f| (f.name.clone(), Expr::col(&f.name)))
            .collect();
        let out = self.push(
            c,
            Op::Project { items },
            vec![dup],
            schema,
            Fidelity::Rewritten,
            Vec::new(),
            Some("DUPLICATE".into()),
        );
        self.set(c, "DUPLICATE", out);
    }

    /// tRowGenerator / tFixedFlowInput: `NB_ROWS` copies of one row of expressions (the
    /// `table` parameter's `value` column per schema column).
    fn row_generator(&mut self, c: &Component, table: &str, value: &str) {
        let fields = schema(c, "");
        let count = param(c, "NB_ROWS").map(|n| self.job.fold(n));
        let Some(count) = count
            .filter(|f| f.complete())
            .and_then(|f| f.text.trim().parse::<u64>().ok())
        else {
            self.gap(
                c,
                "",
                Vec::new(),
                format!("{} row count is known only at run time", c.component_type),
            );
            return;
        };
        let context = context_of(self.job);
        let global = global_of(self.job);
        let column = |_: &str, _: &str| None;
        let none = |_: &str| None;
        let scope = Scope {
            column: &column,
            var: &none,
            context: &context,
            job: &self.job.name,
            global: &global,
        };
        let mut row = Vec::new();
        let mut failure = None;
        let mut fidelity = Fidelity::Exact;
        let mut notes = Vec::new();
        let mut results = Vec::new();
        for f in &fields {
            let text = rows(c, table)
                .iter()
                .find(|r| r.get("SCHEMA_COLUMN") == Some(&f.name))
                .and_then(|r| r.get(value).cloned())
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "null".into());
            let r = lower(&text, &scope);
            results.push(r.as_ref().map(|_| ()).map_err(Clone::clone));
            match r {
                Ok((l, _)) => {
                    fidelity = fidelity.max(l.fidelity);
                    notes.extend(l.notes);
                    row.push(l.expr);
                }
                Err(g) => {
                    failure.get_or_insert(format!("`{}`: {}", f.name, g.reason));
                }
            }
        }
        for r in results {
            self.count_unit(r);
        }
        if let Some(reason) = failure {
            self.gap(c, "", Vec::new(), reason);
            return;
        }
        let id = self.push(
            c,
            Op::Generate { row, count },
            Vec::new(),
            fields,
            fidelity,
            notes,
            None,
        );
        self.set(c, "", id);
    }

    fn aggregate(&mut self, c: &Component) {
        let Some(input) = self.main_input(c) else {
            self.gap(
                c,
                "",
                Vec::new(),
                "tAggregateRow with no row link in".into(),
            );
            return;
        };
        let in_schema = self.input_schema(input);
        let ty_of = |n: &str| {
            in_schema
                .iter()
                .find(|f| f.name == n)
                .map_or(KType::Unknown, |f| f.ty)
        };
        let group_by: Vec<String> = rows(c, "GROUPBYS")
            .iter()
            .filter_map(|r| r.get("INPUT_COLUMN").cloned())
            .collect();
        let mut aggs = Vec::new();
        let mut unknown = Vec::new();
        for r in rows(c, "OPERATIONS") {
            let out = r.get("OUTPUT_COLUMN").cloned().unwrap_or_default();
            let subject = r.get("INPUT_COLUMN").cloned();
            let f = r.get("FUNCTION").map(String::as_str).unwrap_or_default();
            let func = match f {
                "count" | "sum" | "avg" | "min" | "max" | "first" | "last" => f.to_string(),
                "distinct" => "count_distinct".into(),
                "list" => "string_agg".into(),
                other => {
                    unknown.push(other.to_string());
                    continue;
                }
            };
            let ty = match func.as_str() {
                "count" | "count_distinct" => KType::Integer,
                "avg" => KType::Number,
                "string_agg" => KType::String,
                _ => subject.as_deref().map_or(KType::Unknown, ty_of),
            };
            aggs.push(Agg {
                name: out,
                func,
                subject,
                arg: (f == "list").then(|| ",".to_string()),
                ty,
            });
        }
        if !unknown.is_empty() {
            self.gap(
                c,
                "",
                vec![input],
                format!("tAggregateRow function(s) {}", unknown.join(", ")),
            );
            return;
        }
        let mut schema: Vec<Field> = group_by.iter().map(|g| Field::new(g, ty_of(g))).collect();
        schema.extend(aggs.iter().map(|a| Field::new(&a.name, a.ty)));
        let id = self.push(
            c,
            Op::Aggregate { group_by, aggs },
            vec![input],
            schema,
            Fidelity::Exact,
            Vec::new(),
            None,
        );
        self.set(c, "", id);
    }

    /// tJoin: the main flow and a lookup flow, matched on `JOIN_KEY`, the lookup's columns taken
    /// as `LOOKUP_COLS` names them; inner when `USE_INNER_JOIN`.
    fn join(&mut self, c: &Component) {
        let links = inbound(self.job, c);
        let main = links
            .iter()
            .find(|l| l.kind == LinkKind::Main)
            .and_then(|l| self.upstream(l));
        let lookup = links
            .iter()
            .find(|l| l.kind == LinkKind::Lookup)
            .and_then(|l| self.upstream(l));
        let (Some(main), Some(lookup)) = (main, lookup) else {
            self.gap(
                c,
                "",
                Vec::new(),
                "tJoin needs a main and a lookup link".into(),
            );
            return;
        };
        let keys: Vec<(String, String)> = rows(c, "JOIN_KEY")
            .iter()
            .filter_map(|r| {
                Some((
                    r.get("INPUT_COLUMN")?.clone(),
                    r.get("LOOKUP_COLUMN")?.clone(),
                ))
            })
            .collect();
        let lookup_schema = self.input_schema(lookup);
        let values: Vec<LookupValue> = rows(c, "LOOKUP_COLS")
            .iter()
            .filter_map(|r| {
                let name = r.get("LOOKUP_COLUMN")?.clone();
                let ty = lookup_schema
                    .iter()
                    .find(|f| f.name == name)
                    .map_or(KType::Unknown, |f| f.ty);
                Some(LookupValue {
                    rename: r
                        .get("OUTPUT_COLUMN")
                        .cloned()
                        .unwrap_or_else(|| name.clone()),
                    name,
                    default: None,
                    ty,
                })
            })
            .collect();
        let mut schema = self.input_schema(main);
        schema.extend(values.iter().map(|v| Field::new(&v.rename, v.ty)));
        let inner = param(c, "USE_INNER_JOIN") == Some("true");
        let id = self.push(
            c,
            Op::Lookup {
                table: None,
                keys,
                values,
                valid_at: None,
                inner,
            },
            vec![main, lookup],
            schema,
            Fidelity::Rewritten,
            vec![
                "tJoin keeps one lookup row per main row; exact only when the key is unique".into(),
            ],
            None,
        );
        self.set(c, "", id);
    }

    fn sort(&mut self, c: &Component) {
        let Some(input) = self.main_input(c) else {
            self.gap(c, "", Vec::new(), "tSortRow with no row link in".into());
            return;
        };
        let keys = rows(c, "CRITERIA")
            .iter()
            .filter_map(|r| {
                Some(SortKey {
                    field: r.get("COLNAME")?.clone(),
                    ascending: r.get("ORDER").map(String::as_str) != Some("desc"),
                    case_sensitive: true,
                })
            })
            .collect();
        let schema = self.input_schema(input);
        let id = self.push(
            c,
            Op::Sort { keys },
            vec![input],
            schema,
            Fidelity::Exact,
            Vec::new(),
            None,
        );
        self.set(c, "", id);
    }

    fn filter_columns(&mut self, c: &Component) {
        let Some(input) = self.main_input(c) else {
            self.gap(
                c,
                "",
                Vec::new(),
                "tFilterColumns with no row link in".into(),
            );
            return;
        };
        let fields = schema(c, "");
        let items = fields
            .iter()
            .map(|f| (f.name.clone(), Expr::col(&f.name)))
            .collect();
        let id = self.push(
            c,
            Op::Project { items },
            vec![input],
            fields,
            Fidelity::Exact,
            Vec::new(),
            None,
        );
        self.set(c, "", id);
    }

    fn unite(&mut self, c: &Component) {
        let inputs: Vec<NodeId> = inbound(self.job, c)
            .iter()
            .filter_map(|l| self.upstream(l))
            .collect();
        let schema = schema(c, "");
        let id = self.push(
            c,
            Op::Union,
            inputs,
            schema,
            Fidelity::Exact,
            Vec::new(),
            None,
        );
        self.set(c, "", id);
    }

    fn sink(&mut self, c: &Component) {
        let Some(input) = self.main_input(c) else {
            self.gap(c, "", Vec::new(), "output with no row link in".into());
            return;
        };
        let elt = c.logical == "elt_output";
        let table = if elt {
            let p = if param(c, "USE_DIFFERENT_TABLE") == Some("true") {
                "DIFFERENT_TABLE_NAME"
            } else {
                "ELT_TABLE_NAME"
            };
            self.table_ref(c, p, "ELT_SCHEMA_NAME")
        } else {
            self.table_ref(c, "TABLE", "SCHEMA_DB")
        };
        let Some(target) = table else {
            self.gap(c, "", vec![input], "output without a table name".into());
            return;
        };
        let action = param(c, "DATA_ACTION").unwrap_or("INSERT");
        let mode = match action {
            "INSERT" => SinkMode::Insert,
            "UPDATE" => SinkMode::Update,
            "INSERT_OR_UPDATE" | "UPDATE_OR_INSERT" | "UPSERT" | "MERGE" => SinkMode::Upsert,
            "DELETE" => SinkMode::Delete,
            other => {
                self.gap(c, "", vec![input], format!("data action `{other}`"));
                return;
            }
        };
        let fields = schema(c, "");
        let key_cols: Vec<String> = c
            .schemas
            .iter()
            .find(|s| s.connector == "FLOW")
            .or_else(|| c.schemas.first())
            .map(|s| {
                s.columns
                    .iter()
                    .filter(|col| col.key)
                    .map(|col| col.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let column_name = |n: &str| {
            c.schemas
                .iter()
                .flat_map(|s| &s.columns)
                .find(|col| col.name == n)
                .and_then(|col| col.db_name.clone())
                .unwrap_or_else(|| n.to_string())
        };
        let sink = Sink {
            mode,
            columns: fields
                .iter()
                .map(|f| (f.name.clone(), column_name(&f.name)))
                .collect(),
            keys: key_cols
                .iter()
                .map(|k| (k.clone(), column_name(k)))
                .collect(),
            truncate: param(c, "TABLE_ACTION")
                .is_some_and(|a| a.contains("CLEAR") || a.contains("TRUNCATE")),
            scd: None,
            target,
        };
        let (fidelity, notes) = match mode {
            SinkMode::Insert => (Fidelity::Exact, Vec::new()),
            _ => (
                Fidelity::Rewritten,
                vec![format!(
                    "`{action}` changes rows a later run reads: state, not a query (RFC 0268 chooses the dbt shape)"
                )],
            ),
        };
        let id = self.push(
            c,
            Op::Sink { sink },
            vec![input],
            fields,
            fidelity,
            notes,
            None,
        );
        self.set(c, "", id);
    }

    fn file_output(&mut self, c: &Component) {
        let Some(input) = self.main_input(c) else {
            self.gap(c, "", Vec::new(), "file output with no row link in".into());
            return;
        };
        let file = param(c, "FILENAME")
            .map(|f| self.job.fold(f).text)
            .unwrap_or_default();
        let base = file.rsplit(['/', '\\']).next().unwrap_or(&file).to_string();
        let fields = schema(c, "");
        let sink = Sink {
            mode: SinkMode::Insert,
            target: TableRef {
                connection: None,
                schema: Some("files".into()),
                table: base,
            },
            columns: fields
                .iter()
                .map(|f| (f.name.clone(), f.name.clone()))
                .collect(),
            keys: Vec::new(),
            truncate: true,
            scd: None,
        };
        let note = format!("{} writes the file `{file}`", c.component_type);
        let id = self.push(
            c,
            Op::Sink { sink },
            vec![input],
            fields,
            Fidelity::Rewritten,
            vec![note],
            None,
        );
        self.set(c, "", id);
    }

    /// A tMap: every input renamed `<input>__<column>`, a `Lookup` (or `Join`, for
    /// `ALL_MATCHES`) per lookup, then per output an optional `Filter` and a `Project`.
    fn tmap(&mut self, c: &Component, m: &Mapper) {
        if m.inputs.is_empty() {
            self.gap(c, "", Vec::new(), "tMap without inputs".into());
            return;
        }
        let link_of = |name: &str| {
            inbound(self.job, c)
                .into_iter()
                .find(|l| l.label.as_deref() == Some(name))
        };
        let mut prefixed: Vec<(String, NodeId)> = Vec::new();
        for input in &m.inputs {
            let Some(up) = link_of(&input.name).and_then(|l| self.upstream(l)) else {
                for o in &m.outputs {
                    self.gap(
                        c,
                        &o.name,
                        Vec::new(),
                        format!("no link brings tMap input `{}`", input.name),
                    );
                }
                return;
            };
            let fields = self.input_schema(up);
            let items: Vec<(String, Expr)> = fields
                .iter()
                .map(|f| (qualified(&input.name, &f.name), Expr::col(&f.name)))
                .collect();
            let schema: Vec<Field> = fields
                .iter()
                .map(|f| Field {
                    name: qualified(&input.name, &f.name),
                    ..f.clone()
                })
                .collect();
            let id = self.push(
                c,
                Op::Project { items },
                vec![up],
                schema,
                Fidelity::Exact,
                Vec::new(),
                Some(format!("input {}", input.name)),
            );
            prefixed.push((input.name.clone(), id));
        }
        let names: Vec<String> = m.inputs.iter().map(|i| i.name.clone()).collect();
        let vars: Vec<(String, String)> = m
            .vars
            .iter()
            .filter_map(|v| Some((v.name.clone(), v.expression.clone()?)))
            .collect();
        // The combined row: main, then each lookup joined in order.
        let mut core = prefixed[0].1;
        let mut core_left = core;
        let mut notes: Vec<String> = Vec::new();
        let mut inner_keys: Vec<String> = Vec::new();
        for (i, input) in m.inputs.iter().enumerate().skip(1) {
            let lookup_node = prefixed[i].1;
            let lookup_schema = self.input_schema(lookup_node);
            let mut keys = Vec::new();
            let mut bad = None;
            for e in &input.entries {
                let Some(expr) = e.expression.as_deref() else {
                    continue;
                };
                let target = qualified(&input.name, &e.name);
                match simple_ref(expr, &names) {
                    Some((q, col)) => keys.push((qualified(&q, &col), target)),
                    None => bad = Some(format!("tMap join key `{expr}` is not a plain column")),
                }
            }
            if let Some(reason) = bad {
                for o in &m.outputs {
                    self.gap(c, &o.name, vec![core, lookup_node], reason.clone());
                }
                return;
            }
            if keys.is_empty() {
                notes.push(format!(
                    "lookup `{}` has no join key: every main row meets every lookup row",
                    input.name
                ));
            }
            let mode = input.matching_mode.as_deref().unwrap_or("UNIQUE_MATCH");
            let combined = |b: &Self, left: NodeId| {
                let mut s = b.input_schema(left);
                s.extend(lookup_schema.iter().cloned());
                s
            };
            if input.inner_join {
                inner_keys.extend(keys.iter().map(|(_, l)| l.clone()));
            }
            if mode == "ALL_MATCHES" {
                let kind = if input.inner_join {
                    JoinKind::Inner
                } else {
                    JoinKind::Left
                };
                let schema = combined(self, core);
                core = self.push(
                    c,
                    Op::Join {
                        kind,
                        keys: keys.clone(),
                        condition: None,
                    },
                    vec![core, lookup_node],
                    schema,
                    Fidelity::Exact,
                    Vec::new(),
                    Some(format!("lookup {}", input.name)),
                );
                let schema = combined(self, core_left);
                core_left = self.push(
                    c,
                    Op::Join {
                        kind: JoinKind::Left,
                        keys,
                        condition: None,
                    },
                    vec![core_left, lookup_node],
                    schema,
                    Fidelity::Exact,
                    Vec::new(),
                    Some(format!("lookup {} (left)", input.name)),
                );
                continue;
            }
            let which = if mode == "FIRST_MATCH" {
                "first"
            } else {
                "last"
            };
            let note = format!(
                "lookup `{}` ({mode}): Talend keeps the {which} matching row; the SQL keeps one — exact only when the key is unique",
                input.name
            );
            let values: Vec<LookupValue> = lookup_schema
                .iter()
                .map(|f| LookupValue {
                    name: f.name.clone(),
                    rename: f.name.clone(),
                    default: None,
                    ty: f.ty,
                })
                .collect();
            let schema = combined(self, core);
            core = self.push(
                c,
                Op::Lookup {
                    table: None,
                    keys: keys.clone(),
                    values: values.clone(),
                    valid_at: None,
                    inner: input.inner_join,
                },
                vec![core, lookup_node],
                schema,
                Fidelity::Rewritten,
                vec![note.clone()],
                Some(format!("lookup {}", input.name)),
            );
            let schema = combined(self, core_left);
            core_left = self.push(
                c,
                Op::Lookup {
                    table: None,
                    keys,
                    values,
                    valid_at: None,
                    inner: false,
                },
                vec![core_left, lookup_node],
                schema,
                Fidelity::Rewritten,
                vec![note],
                Some(format!("lookup {} (left)", input.name)),
            );
        }
        let filters: Vec<(String, Option<String>)> = m
            .outputs
            .iter()
            .map(|o| (o.name.clone(), o.filter.clone()))
            .collect();
        for o in &m.outputs {
            self.tmap_output(
                c,
                o,
                &names,
                &vars,
                &filters,
                core,
                core_left,
                &inner_keys,
                &notes,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tmap_output(
        &mut self,
        c: &Component,
        o: &MapperOutput,
        names: &[String],
        vars: &[(String, String)],
        filters: &[(String, Option<String>)],
        core: NodeId,
        core_left: NodeId,
        inner_keys: &[String],
        notes: &[String],
    ) {
        let base = if o.reject_inner_join { core_left } else { core };
        let fields = self.input_schema(base);
        let column = |q: &str, col: &str| {
            names
                .iter()
                .any(|n| n == q)
                .then(|| fields.iter().find(|f| f.name == qualified(q, col)))
                .flatten()
                .map(|f| (Expr::col(&f.name), f.ty))
        };
        let var = |v: &str| vars.iter().find(|(n, _)| n == v).map(|(_, e)| e.clone());
        let context = context_of(self.job);
        let global = global_of(self.job);
        let scope = Scope {
            column: &column,
            var: &var,
            context: &context,
            job: &self.job.name,
            global: &global,
        };
        let mut fidelity = Fidelity::Exact;
        let mut out_notes: Vec<String> = notes.to_vec();
        let mut predicate: Option<Expr> = None;
        let mut failure: Option<String> = None;
        let mut counted = Vec::new();
        if o.reject_inner_join {
            let missing: Vec<Expr> = inner_keys
                .iter()
                .map(|k| Expr::is_null(Expr::col(k)))
                .collect();
            if let Some(first) = missing.first().cloned() {
                predicate = Some(missing.into_iter().skip(1).fold(first, Expr::or));
                out_notes.push(
                    "the inner-join reject output takes the main rows no inner lookup matched"
                        .into(),
                );
                fidelity = Fidelity::Rewritten;
            }
        }
        if o.reject {
            let others: Vec<&String> = filters
                .iter()
                .filter(|(n, _)| n != &o.name)
                .filter_map(|(_, f)| f.as_ref())
                .collect();
            let unfiltered = filters.iter().any(|(n, f)| n != &o.name && f.is_none());
            if unfiltered {
                predicate = Some(Expr::bool(false));
                out_notes.push("every row is accepted by an output without a filter, so the reject output receives none".into());
            } else {
                let mut parts = Vec::new();
                for f in others {
                    let r = lower(f, &scope);
                    counted.push(r.as_ref().map(|_| ()).map_err(|g| g.clone()));
                    match r {
                        Ok((l, _)) => parts.push(Expr::two_valued(l.expr)),
                        Err(g) => failure = Some(g.reason),
                    }
                }
                if let Some(first) = parts.first().cloned() {
                    let any = parts.into_iter().skip(1).fold(first, Expr::or);
                    predicate = Some(Expr::Unary {
                        op: ekos_lakehouse_etl_expr::UnOp::Not,
                        arg: Box::new(any),
                    });
                }
            }
            out_notes.push(
                "the reject output receives the rows no other output's filter accepted".into(),
            );
            fidelity = Fidelity::Rewritten;
        }
        if let Some(f) = &o.filter {
            let r = lower(f, &scope);
            counted.push(r.as_ref().map(|_| ()).map_err(|g| g.clone()));
            match r {
                Ok((l, _)) => {
                    fidelity = fidelity.max(l.fidelity);
                    out_notes.extend(l.notes);
                    let p = Expr::two_valued(l.expr);
                    predicate = Some(match predicate {
                        Some(q) => Expr::and(q, p),
                        None => p,
                    });
                }
                Err(g) => failure = Some(g.reason),
            }
        }
        let mut items = Vec::new();
        let mut schema = Vec::new();
        for e in &o.entries {
            let text = e.expression.as_deref().unwrap_or("null");
            let r = lower(text, &scope);
            counted.push(r.as_ref().map(|_| ()).map_err(|g| g.clone()));
            match r {
                Ok((l, ty)) => {
                    fidelity = fidelity.max(l.fidelity);
                    for n in l.notes {
                        if !out_notes.contains(&n) {
                            out_notes.push(n);
                        }
                    }
                    let declared = talend_type(e.talend_type.as_deref());
                    schema.push(Field::new(
                        &e.name,
                        if declared == KType::Unknown {
                            ty
                        } else {
                            declared
                        },
                    ));
                    items.push((e.name.clone(), l.expr));
                }
                Err(g) => {
                    failure.get_or_insert(format!("`{}`: {}", e.name, g.reason));
                }
            }
        }
        for r in counted {
            self.count_unit(r);
        }
        if let Some(reason) = failure {
            self.gap(c, &o.name, vec![base], reason);
            return;
        }
        let mut head = base;
        if let Some(p) = predicate {
            let s = self.input_schema(base);
            head = self.push(
                c,
                Op::Filter { predicate: p },
                vec![base],
                s,
                fidelity,
                Vec::new(),
                Some(format!("output {}", o.name)),
            );
        }
        let id = self.push(
            c,
            Op::Project { items },
            vec![head],
            schema,
            fidelity,
            out_notes,
            Some(o.name.clone()),
        );
        self.set(c, &o.name, id);
    }
}

/// `context.<name>`: the job's default value and its type.
fn context_of(job: &Job) -> impl Fn(&str) -> Option<(Option<String>, KType)> + '_ {
    move |name: &str| {
        let var = job.context_var(name)?;
        Some((var.value.clone(), talend_type(Some(&var.talend_type))))
    }
}

/// What a `globalMap` key stands for: a constant every `tSetGlobalVar` of the job sets it to, or
/// the file a `tFileList` is on (`<list>_CURRENT_FILEPATH` / `_CURRENT_FILE`), read from the
/// landed table's `$path` column.
fn global_of(job: &Job) -> impl Fn(&str) -> Option<(Expr, KType, String)> + '_ {
    let constants = job.global_constants();
    move |key: &str| {
        if let Some(v) = constants.get(key) {
            return Some((
                Expr::str(v),
                KType::String,
                format!("`globalMap` key `{key}` is set to a constant by tSetGlobalVar"),
            ));
        }
        let list = key
            .strip_suffix("_CURRENT_FILEPATH")
            .or_else(|| key.strip_suffix("_CURRENT_FILE"))?;
        job.component(list)
            .filter(|c| c.component_type == "tFileList")?;
        let path = Expr::col("$path");
        let note = format!(
            "`{key}` is the file {list} is on; read from the landed table's `$path` (the file each row came from)"
        );
        Some(if key.ends_with("_CURRENT_FILEPATH") {
            (path, KType::String, note)
        } else {
            (
                Expr::call(
                    ekos_lakehouse_etl_expr::Func::RegexpReplace,
                    vec![path, Expr::str("^.*/"), Expr::str("")],
                ),
                KType::String,
                note,
            )
        })
    }
}

/// `row1.COL` (one of `inputs`), nothing else: a join key the IR can name.
fn simple_ref(expr: &str, inputs: &[String]) -> Option<(String, String)> {
    let e = expr.trim();
    let (q, col) = e.split_once('.')?;
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (inputs.iter().any(|i| i == q) && ident(col)).then(|| (q.to_string(), col.to_string()))
}

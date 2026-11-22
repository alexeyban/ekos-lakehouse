//! A Talend job's data components as nodes of the shared relational IR (EKOS RFC 0267
//! §Components). Pure: the job (joblets already inlined) and a catalogue in, an [`Ir`] and what
//! was left out of the data flow out.

use std::collections::{BTreeMap, HashMap};

use ekos_lakehouse_etl_expr::{BinOp, Expr, Fidelity, Gap, KType, Lowered};
use ekos_lakehouse_etl_rel::{
    Agg, Catalog, Field, Ir, JoinKind, LookupValue, NodeId, Op, RelNode, Sink, SinkMode, SortKey,
    SourceQuery, TableRef,
};
use ekos_talend::{
    Category, Component, EltInput, EltMap, EltOutput, Job, Link, LinkKind, Mapper, MapperOutput,
    Param, Routine,
};

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
    "tSleep",
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
    /// Context variables a `tJavaRow` sets from a row (`context.v = input_row.c;`): known only at
    /// run time, the watermark pattern (RFC 0267 phase 2 §tJavaRow and run-time context).
    pub context_from_data: Vec<ContextFromData>,
    /// Gaps a model may draft (RFC 0268 phase 3).
    pub draftable: Vec<Draftable>,
}

/// `context.<var> = input_row.<column>;` in `component`, whose rows come from node `from`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFromData {
    pub var: String,
    pub component: String,
    pub column: String,
    pub from: Option<NodeId>,
}

/// What the operator states about the run that the project does not (EKOS RFC 0268 phase 2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// The JVM's default locale ([`Locale::parse`]). Unstated, what depends on it (week numbers,
    /// month and day names) is a gap.
    pub locale: Option<Locale>,
    /// Checked model drafts (RFC 0205 through RFC 0268 phase 3), by `(job, component, port)`: the
    /// gap there becomes the draft, `llm-drafted`.
    pub drafts: std::collections::BTreeMap<(String, String, String), ekos_lakehouse_etl_rel::Draft>,
}

/// A gap a model may draft: one component's output from one input whose fields are all known,
/// with the component's own text (its code, or the tMap output's expressions).
#[derive(Debug, Clone, PartialEq)]
pub struct Draftable {
    pub job: String,
    pub component: String,
    /// The output port (a tMap output's name), or empty.
    pub port: String,
    pub component_type: String,
    pub original: String,
    pub inputs: Vec<Field>,
    pub outputs: Vec<Field>,
    pub reason: String,
}

/// A JVM locale the lowering knows: its week rule (from the country) and its month and day
/// names (from the language, CLDR's, as Java 9+ prints them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locale {
    pub tag: &'static str,
    /// Weeks start on Monday and the first has four days: Java's `WEEK_OF_YEAR` is the ISO week.
    pub iso_weeks: bool,
    pub months: &'static [&'static str; 12],
    /// Sunday first, as `DAY_OF_WEEK` counts.
    pub days: &'static [&'static str; 7],
}

const MONTHS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];
const DAYS_FR: [&str; 7] = [
    "dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi",
];
const MONTHS_DE: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];
const DAYS_DE: [&str; 7] = [
    "Sonntag",
    "Montag",
    "Dienstag",
    "Mittwoch",
    "Donnerstag",
    "Freitag",
    "Samstag",
];
const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS_EN: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS_ES: [&str; 12] = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
];
const DAYS_ES: [&str; 7] = [
    "domingo",
    "lunes",
    "martes",
    "miércoles",
    "jueves",
    "viernes",
    "sábado",
];
const MONTHS_IT: [&str; 12] = [
    "gennaio",
    "febbraio",
    "marzo",
    "aprile",
    "maggio",
    "giugno",
    "luglio",
    "agosto",
    "settembre",
    "ottobre",
    "novembre",
    "dicembre",
];
const DAYS_IT: [&str; 7] = [
    "domenica",
    "lunedì",
    "martedì",
    "mercoledì",
    "giovedì",
    "venerdì",
    "sabato",
];
const MONTHS_NL: [&str; 12] = [
    "januari",
    "februari",
    "maart",
    "april",
    "mei",
    "juni",
    "juli",
    "augustus",
    "september",
    "oktober",
    "november",
    "december",
];
const DAYS_NL: [&str; 7] = [
    "zondag",
    "maandag",
    "dinsdag",
    "woensdag",
    "donderdag",
    "vrijdag",
    "zaterdag",
];

impl Locale {
    /// The locales the lowering knows.
    pub const KNOWN: &'static [&'static str] = &[
        "fr_FR", "fr_BE", "fr_CH", "de_DE", "de_AT", "de_CH", "en_GB", "en_IE", "en_US", "es_ES",
        "it_IT", "nl_NL", "nl_BE",
    ];

    /// `fr_FR`, `de-DE`, …: `None` for a locale the lowering does not know.
    pub fn parse(tag: &str) -> Option<Locale> {
        let t = tag.replace('-', "_");
        let tag = *Self::KNOWN.iter().find(|k| k.eq_ignore_ascii_case(&t))?;
        let (lang, country) = tag.split_once('_')?;
        let (months, days) = match lang {
            "fr" => (&MONTHS_FR, &DAYS_FR),
            "de" => (&MONTHS_DE, &DAYS_DE),
            "en" => (&MONTHS_EN, &DAYS_EN),
            "es" => (&MONTHS_ES, &DAYS_ES),
            "it" => (&MONTHS_IT, &DAYS_IT),
            _ => (&MONTHS_NL, &DAYS_NL),
        };
        Some(Locale {
            tag,
            // The United States count weeks from Sunday, the first having one day.
            iso_weeks: country != "US",
            months,
            days,
        })
    }
}

/// Lowers one job (expand joblets first: `Project::expand`). `routines` are the project's
/// (`Project::routines`): a custom routine whose body is a digest is lowered.
pub fn lower_job(job: &Job, routines: &[Routine], catalog: &dyn Catalog) -> Lowering {
    lower_job_with(job, routines, catalog, &Options::default())
}

/// [`lower_job`] with what the operator states about the run.
pub fn lower_job_with(
    job: &Job,
    routines: &[Routine],
    catalog: &dyn Catalog,
    options: &Options,
) -> Lowering {
    let mut b = Builder {
        job,
        routines,
        catalog,
        options: options.clone(),
        out: Lowering {
            ir: Ir {
                transformation: job.path.clone(),
                nodes: Vec::new(),
            },
            ..Lowering::default()
        },
        ports: HashMap::new(),
        writers: context_writers(job),
        runtime: ekos_talend::RuntimeContext::of(job),
    };
    // Data components, and any other component a row link reaches or leaves (a `tJavaRow`, a
    // `tJavaFlex`): it is part of the flow, lowered or a gap.
    let on_a_row_link = |c: &Component| {
        job.links.iter().any(|l| {
            (l.to == c.unique_name || l.from == c.unique_name)
                && matches!(
                    l.kind,
                    LinkKind::Main
                        | LinkKind::Lookup
                        | LinkKind::Filter
                        | LinkKind::Reject
                        | LinkKind::Duplicate
                )
        })
    };
    let data: Vec<&Component> = job
        .components
        .iter()
        .filter(|c| {
            c.category.is_data()
                || PASS_THROUGH.contains(&c.component_type.as_str())
                || c.component_type == "tJavaRow"
                || on_a_row_link(c)
        })
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

/// The landed table's column naming the file each row came from.
const PATH: &str = "$path";

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
    routines: &'a [Routine],
    catalog: &'a dyn Catalog,
    options: Options,
    out: Lowering,
    /// `(component, port)` → the node rows leave from. Port `""` is the main output; a tMap's
    /// outputs are their names; `REJECT` / `DUPLICATE` the second outputs.
    ports: HashMap<(String, String), NodeId>,
    /// `context.<var>` → the `tJavaRow` and input column that set it from a row.
    writers: BTreeMap<String, (String, String)>,
    runtime: ekos_talend::RuntimeContext,
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
        if let [input] = inputs.as_slice()
            && let Some(id) = self.drafted(c, port, *input, &schema, &reason)
        {
            return id;
        }
        if let [input] = inputs.as_slice() {
            self.offer_draft(c, port, *input, &schema, &reason);
        }
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

    /// The text a model drafts from: a code component's code, or a tMap output's filter and
    /// expressions (with the map's variables). `None` for a component that is neither.
    fn original_text(&self, c: &Component, port: &str) -> Option<String> {
        if let Some(m) = &c.mapper {
            let o = m.outputs.iter().find(|o| o.name == port)?;
            let mut t = String::new();
            for v in &m.vars {
                t.push_str(&format!(
                    "Var.{} = {}\n",
                    v.name,
                    v.expression.as_deref().unwrap_or("")
                ));
            }
            if let Some(f) = &o.filter {
                t.push_str(&format!("filter: {f}\n"));
            }
            for e in &o.entries {
                t.push_str(&format!(
                    "{} = {}\n",
                    e.name,
                    e.expression.as_deref().unwrap_or("")
                ));
            }
            return Some(t);
        }
        let code: Vec<&str> = ["CODE", "CODE_START", "CODE_MAIN", "CODE_END"]
            .iter()
            .filter_map(|k| param(c, k))
            .collect();
        (!code.is_empty()).then(|| code.join("\n// ---\n"))
    }

    fn offer_draft(
        &mut self,
        c: &Component,
        port: &str,
        input: NodeId,
        outputs: &[Field],
        reason: &str,
    ) {
        let n = &self.out.ir.nodes[input];
        if n.is_gap() || outputs.is_empty() || n.schema.iter().any(|f| f.ty == KType::Unknown) {
            return;
        }
        let Some(original) = self.original_text(c, port) else {
            return;
        };
        self.out.draftable.push(Draftable {
            job: self.job.name.clone(),
            component: c.unique_name.clone(),
            port: port.to_string(),
            component_type: c.component_type.clone(),
            original,
            inputs: n.schema.clone(),
            outputs: outputs.to_vec(),
            reason: reason.to_string(),
        });
    }

    /// The draft for this gap, when the options hold one: the rows kept, then each declared output
    /// from its drafted expression (RFC 0205's form, `llm-drafted`).
    fn drafted(
        &mut self,
        c: &Component,
        port: &str,
        input: NodeId,
        outputs: &[Field],
        reason: &str,
    ) -> Option<NodeId> {
        let d = self
            .options
            .drafts
            .get(&(
                self.job.name.clone(),
                c.unique_name.clone(),
                port.to_string(),
            ))?
            .clone();
        let items: Vec<(String, Expr)> = outputs
            .iter()
            .map(|f| {
                let (_, sql) = d.columns.iter().find(|(n, _)| *n == f.name)?;
                Some((
                    f.name.clone(),
                    Expr::Sql {
                        text: sql.clone(),
                        ty: f.ty,
                    },
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        let notes = vec![
            format!("drafted by {} (the translator could not follow it: {reason})", d.model),
            "checked: parses as Trino, reads only the component's input fields, functions on the list, literals from its text".into(),
        ];
        let branch = (!port.is_empty()).then(|| port.to_string());
        let mut head = input;
        if let Some(p) = &d.keep_when {
            let schema = self.out.ir.nodes[input].schema.clone();
            head = self.push(
                c,
                Op::Filter {
                    predicate: Expr::Sql {
                        text: p.clone(),
                        ty: KType::Boolean,
                    },
                },
                vec![input],
                schema,
                Fidelity::LlmDrafted,
                notes.clone(),
                branch.clone(),
            );
        }
        let id = self.push(
            c,
            Op::Project { items },
            vec![head],
            outputs.to_vec(),
            Fidelity::LlmDrafted,
            notes,
            branch,
        );
        self.ports
            .insert((c.unique_name.clone(), port.to_string()), id);
        Some(id)
    }

    fn set(&mut self, c: &Component, port: &str, id: NodeId) {
        self.ports
            .insert((c.unique_name.clone(), port.to_string()), id);
    }

    /// The node a link brings rows from.
    fn upstream(&self, l: &Link) -> Option<NodeId> {
        let from = self.job.component(&l.from)?;
        let port = if from.mapper.is_some() || from.elt.is_some() {
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
        // A component no row link reaches or leaves is disconnected: not part of any flow, so not
        // a translation gap.
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
        // A component that sends rows on (a `tJavaFlex` under a `tLoop`, a `tIterateToFlow`, a CDC
        // reader) produces them: part of the flow, lowered or a gap.
        let sends_rows = self.job.links.iter().any(|l| {
            l.from == c.unique_name
                && matches!(
                    l.kind,
                    LinkKind::Main
                        | LinkKind::Lookup
                        | LinkKind::Filter
                        | LinkKind::Reject
                        | LinkKind::Duplicate
                )
        });
        if !source && !statement && !sends_rows && inbound(self.job, c).is_empty() {
            self.out.skipped.push((
                c.unique_name.clone(),
                format!("{}: no row link in, not part of any flow", c.component_type),
            ));
            return;
        }
        if let Some(m) = &c.mapper {
            return self.tmap(c, m);
        }
        if let Some(e) = &c.elt {
            return self.elt_map(c, e);
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
            "db_row" => self.statement(c),
            "elt_input" => self.elt_input(c),
            l if l.starts_with("db_output") || l == "elt_output" => self.sink(c),
            _ => match t {
                "tFilterRow" => self.filter_row(c),
                "tJavaRow" => self.java_row(c),
                "tSampleRow" => self.sample_row(c),
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

    /// The database connection a component reads or writes through: its engine, and the folded
    /// `DBNAME` and `SCHEMA_DB`, from the component or the `tDBConnection` its `CONNECTION` names
    /// (EKOS RFC 0268). No host, user or password is ever read.
    fn connection(&self, c: &Component) -> Option<ekos_lakehouse_etl_rel::Connection> {
        let shared = param(c, "CONNECTION").and_then(|n| self.job.component(n));
        let engine = c
            .engine
            .clone()
            .or_else(|| shared.and_then(|s| s.engine.clone()))?;
        // The component's own value, or the shared connection's when the component leaves it
        // empty (a component on an existing connection keeps `""` in its own field).
        let folded = |name: &str| {
            let of = |x: &Component| {
                param(x, name)
                    .map(|v| self.job.fold(v))
                    .filter(|f| f.complete() && !f.text.is_empty())
                    .map(|f| f.text)
            };
            of(c).or_else(|| shared.and_then(of))
        };
        Some(ekos_lakehouse_etl_rel::Connection {
            name: shared.map_or_else(|| c.unique_name.clone(), |s| s.unique_name.clone()),
            db_type: engine.to_ascii_uppercase(),
            access: String::new(),
            host: None,
            database: folded("DBNAME"),
            port: None,
            username: None,
            schema: folded("SCHEMA_DB"),
        })
    }

    fn table_ref(&self, c: &Component, table_param: &str, schema_param: &str) -> Option<TableRef> {
        let table = self.job.fold(param(c, table_param)?).text;
        let fold = |v: Option<&str>| {
            v.map(|s| self.job.fold(s))
                .filter(|f| f.complete() && !f.text.is_empty())
                .map(|f| f.text)
        };
        let schema = fold(param(c, schema_param)).or_else(|| {
            fold(
                param(c, "CONNECTION")
                    .and_then(|n| self.job.component(n))
                    .and_then(|conn| param(conn, "SCHEMA_DB")),
            )
        });
        (!table.is_empty()).then_some(TableRef {
            connection: self.connection(c),
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
        if let Some(q) = param(c, "QUERY") {
            let run_time = self.run_time_notes(q);
            if !run_time.is_empty() {
                fidelity = fidelity.max(Fidelity::Rewritten);
                notes.extend(run_time);
            }
        }
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

    /// The folded masks of the `tFileList` whose current file `filename` reads, joined by `|`;
    /// `None` when it reads none, or a mask does not fold.
    fn list_masks(&self, filename: &str) -> Option<String> {
        let start = filename.find("globalMap.get(\"")? + "globalMap.get(\"".len();
        let key = &filename[start..];
        let key = &key[..key.find('"')?];
        let list = key
            .strip_suffix("_CURRENT_FILEPATH")
            .or_else(|| key.strip_suffix("_CURRENT_FILE"))?;
        let c = self
            .job
            .component(list)
            .filter(|c| c.component_type == "tFileList")?;
        let masks: Vec<String> = rows(c, "FILES")
            .iter()
            .filter_map(|r| r.get("FILEMASK"))
            .map(|m| self.job.fold(m))
            .map(|f| f.complete().then_some(f.text))
            .collect::<Option<Vec<_>>>()?;
        (!masks.is_empty()).then(|| masks.join("|"))
    }

    fn file_input(&mut self, c: &Component) {
        let mut fields = schema(c, "");
        // A file a `tFileList` names (`globalMap.get("…_CURRENT_FILEPATH")`): the landed table's
        // `$path` says which file each row came from (RFC 0267 §globalMap keys).
        if param(c, "FILENAME").is_some_and(|f| f.contains("globalMap"))
            && !fields.iter().any(|f| f.name == PATH)
        {
            fields.push(Field::new(PATH, KType::String));
        }
        let file = param(c, "FILENAME")
            .map(|f| self.job.fold(f).text)
            .unwrap_or_default();
        // The current file of a `tFileList`: every file its masks match, so the landed table is
        // named by those masks (`ICOM_*_TYPE_CLIENT.csv`), not by the expression, which is the same
        // text in every job that iterates a list (EKOS RFC 0268 phase 3).
        let base = self
            .list_masks(param(c, "FILENAME").unwrap_or_default())
            .unwrap_or_else(|| file.rsplit(['/', '\\']).next().unwrap_or(&file).to_string());
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
                        routines: self.routines,
                        locale: self.options.locale,
                        sequence: &no_sequence,
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
        let mut count_notes: Vec<String> = Vec::new();
        let count = match param(c, "NB_ROWS").and_then(|n| set_by_tjava(self.job, n)) {
            Some(Ok((n, note))) => {
                count_notes.push(note);
                Some(n)
            }
            Some(Err(reason)) => {
                self.gap(c, "", Vec::new(), reason);
                return;
            }
            None => param(c, "NB_ROWS")
                .map(|n| self.job.fold(n))
                .filter(|f| f.complete())
                .and_then(|f| f.text.trim().parse::<u64>().ok()),
        };
        let Some(count) = count else {
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
        // Numeric.sequence: the generated rows are all alike before they are numbered, so
        // numbering them in any order gives the same rows.
        let mut seqs: Vec<(String, Option<(i64, i64)>)> = Vec::new();
        for r in rows(c, table) {
            if let Some(t) = r.get(value) {
                for q in crate::expr::sequences(t) {
                    if !seqs.iter().any(|(n, _)| *n == q.0) {
                        seqs.push(q);
                    }
                }
            }
        }
        let users = sequence_users(self.job);
        for (name, nums) in &seqs {
            let n = users.get(name).map_or(0, Vec::len);
            if n > 1 || nums.is_none() {
                let reason = if n > 1 {
                    format!(
                        "Numeric.sequence(\"{name}\") is called {n} times in the job: each call takes the next number of one counter"
                    )
                } else {
                    format!("Numeric.sequence(\"{name}\") with a computed start or step")
                };
                self.count_unit(Err(Gap::new(reason.clone())));
                self.gap(c, "", Vec::new(), reason);
                return;
            }
        }
        let known: Vec<String> = seqs.iter().map(|(n, _)| n.clone()).collect();
        let sequence = |name: &str| {
            known
                .iter()
                .any(|n| n == name)
                .then(|| Expr::col(&format!("__seq_{name}")))
        };
        let scope = Scope {
            column: &column,
            var: &none,
            context: &context,
            job: &self.job.name,
            global: &global,
            routines: self.routines,
            locale: self.options.locale,
            sequence: &sequence,
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
        let count_fidelity = if count_notes.is_empty() {
            Fidelity::Exact
        } else {
            Fidelity::Rewritten
        };
        if seqs.is_empty() {
            notes.extend(count_notes);
            let id = self.push(
                c,
                Op::Generate { row, count },
                Vec::new(),
                fields,
                fidelity.max(count_fidelity),
                notes,
                None,
            );
            self.set(c, "", id);
            return;
        }
        // Rows first, numbered, then the columns over the numbers.
        let gen_field = Field::new("__generated", KType::Integer);
        let g = self.push(
            c,
            Op::Generate {
                row: vec![Expr::int(1)],
                count,
            },
            Vec::new(),
            vec![gen_field.clone()],
            count_fidelity,
            count_notes,
            None,
        );
        let mut numbered = vec![gen_field];
        let mut items = Vec::new();
        for (name, nums) in &seqs {
            let (start, increment) = nums.unwrap_or((1, 1));
            let col = format!("__seq_{name}");
            numbered.push(Field::new(&col, KType::Integer));
            items.push((
                col,
                ekos_lakehouse_etl_rel::WindowFn::Sequence { start, increment },
            ));
        }
        let w = self.push(
            c,
            Op::Window {
                partition: Vec::new(),
                order: Vec::new(),
                items,
            },
            vec![g],
            numbered,
            Fidelity::Exact,
            vec!["the generated rows are alike before they are numbered: any order gives the same rows".into()],
            None,
        );
        let items = fields
            .iter()
            .zip(row)
            .map(|(f, e)| (f.name.clone(), e))
            .collect();
        let id = self.push(
            c,
            Op::Project { items },
            vec![w],
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
        // The order rows arrive in, when the main input comes straight from a tSortRow.
        let main_order: Option<(String, Vec<SortKey>)> = self
            .out
            .ir
            .nodes
            .get(prefixed[0].1)
            .and_then(|p| p.inputs.first())
            .map(|&up| &self.out.ir.nodes[up])
            .and_then(|n| match &n.op {
                Op::Sort { keys } => Some((
                    n.step.clone(),
                    keys.iter()
                        .map(|k| SortKey {
                            field: qualified(&names[0], &k.field),
                            ..k.clone()
                        })
                        .collect(),
                )),
                _ => None,
            });
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
                main_order.as_ref(),
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
        main_order: Option<&(String, Vec<SortKey>)>,
    ) {
        let base = if o.reject_inner_join { core_left } else { core };
        // Numeric.sequence: a numbered column per sequence; over every row when a variable or the
        // filter reads it, over the output's rows when only its columns do.
        let mut early: Vec<(String, Option<(i64, i64)>)> = Vec::new();
        for (_, text) in vars {
            for q in crate::expr::sequences(text) {
                if !early.iter().any(|(n, _)| *n == q.0) {
                    early.push(q);
                }
            }
        }
        if let Some(f) = &o.filter {
            for q in crate::expr::sequences(f) {
                if !early.iter().any(|(n, _)| *n == q.0) {
                    early.push(q);
                }
            }
        }
        let mut late: Vec<(String, Option<(i64, i64)>)> = Vec::new();
        for e in &o.entries {
            for q in crate::expr::sequences(e.expression.as_deref().unwrap_or_default()) {
                if !early.iter().chain(&late).any(|(n, _)| *n == q.0) {
                    late.push(q);
                }
            }
        }
        let users = sequence_users(self.job);
        for (name, nums) in early.iter().chain(&late) {
            let n = users.get(name).map_or(0, Vec::len);
            let reason = if n > 1 {
                Some(format!(
                    "Numeric.sequence(\"{name}\") is called {n} times in the job: each call takes the next number of one counter"
                ))
            } else if nums.is_none() {
                Some(format!(
                    "Numeric.sequence(\"{name}\") with a computed start or step"
                ))
            } else {
                None
            };
            if let Some(reason) = reason {
                self.count_unit(Err(Gap::new(reason.clone())));
                self.gap(c, &o.name, vec![base], reason);
                return;
            }
        }
        let seq_note = match main_order {
            Some((step, _)) => format!(
                "Numeric.sequence numbers rows in {step}'s order; rows equal on its keys may be numbered either way"
            ),
            None => "Numeric.sequence numbers rows in arrival order, which SQL does not keep: the order must be stated".into(),
        };
        let order: Vec<SortKey> = main_order.map(|(_, k)| k.clone()).unwrap_or_default();
        let window = |b: &mut Self, over: NodeId, seqs: &[(String, Option<(i64, i64)>)]| {
            if seqs.is_empty() {
                return over;
            }
            let mut schema = b.input_schema(over);
            let mut items = Vec::new();
            for (name, nums) in seqs {
                let (start, increment) = nums.unwrap_or((1, 1));
                let col = format!("__seq_{name}");
                schema.push(Field::new(&col, KType::Integer));
                items.push((
                    col,
                    ekos_lakehouse_etl_rel::WindowFn::Sequence { start, increment },
                ));
            }
            b.push(
                c,
                Op::Window {
                    partition: Vec::new(),
                    order: order.clone(),
                    items,
                },
                vec![over],
                schema,
                Fidelity::Rewritten,
                vec![seq_note.clone()],
                Some(format!("output {}", o.name)),
            )
        };
        let base = window(self, base, &early);
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
        let plain_global = global_of(self.job);
        // In the combined row, `$path` is the main input's: `<input>__$path`.
        let path_field = fields
            .iter()
            .find(|f| f.name == qualified(&names[0], PATH))
            .map(|f| f.name.clone());
        let global = |key: &str| {
            plain_global(key).map(|(e, t, note)| {
                let e = e.map_columns(&|c| {
                    if c == PATH {
                        Expr::col(path_field.as_deref().unwrap_or(PATH))
                    } else {
                        Expr::col(c)
                    }
                });
                (e, t, note)
            })
        };
        let known: Vec<String> = early.iter().chain(&late).map(|(n, _)| n.clone()).collect();
        let sequence = |name: &str| {
            known
                .iter()
                .any(|n| n == name)
                .then(|| Expr::col(&format!("__seq_{name}")))
        };
        let scope = Scope {
            column: &column,
            var: &var,
            context: &context,
            job: &self.job.name,
            global: &global,
            routines: self.routines,
            locale: self.options.locale,
            sequence: &sequence,
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
        if !early.is_empty() || !late.is_empty() {
            fidelity = fidelity.max(Fidelity::Rewritten);
            out_notes.push(seq_note.clone());
        }
        let head = window(self, head, &late);
        // The file each row came from travels with the row (EKOS RFC 0268 phase 2): a later
        // component's `globalMap.get("…_CURRENT_FILEPATH")` reads it from its own input.
        if let Some(p) = &path_field
            && !items.iter().any(|(n, _)| n == PATH)
        {
            items.push((PATH.to_string(), Expr::col(p)));
            schema.push(Field::new(PATH, KType::String));
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

/// The columns of a combined ELT / tMap row: `<input>.<column>` is the field
/// `<input>__<column>`; an unqualified name is the one field of that column, when only one has it.
fn combined_lookup(fields: Vec<Field>) -> impl Fn(&str, &str) -> Option<(Expr, KType)> {
    move |q: &str, col: &str| {
        let hit = |f: &&Field| {
            if q.is_empty() {
                f.name.ends_with(&format!("__{col}"))
            } else {
                f.name == qualified(q, col)
            }
        };
        let exact: Vec<&Field> = fields.iter().filter(hit).collect();
        // Unquoted names are case-insensitive in SQL: a different case is the same column, when
        // only one column matches that way.
        let found = if exact.is_empty() {
            let want = if q.is_empty() {
                format!("__{col}").to_ascii_lowercase()
            } else {
                qualified(q, col).to_ascii_lowercase()
            };
            fields
                .iter()
                .filter(|f| {
                    let n = f.name.to_ascii_lowercase();
                    if q.is_empty() {
                        n.ends_with(&want)
                    } else {
                        n == want
                    }
                })
                .collect()
        } else {
            exact
        };
        match found.as_slice() {
            [f] => Some((Expr::col(&f.name), f.ty)),
            _ => None,
        }
    }
}

/// The input of a lowered SQL expression that names one column only, and that column.
fn plain_column(e: &Expr) -> Option<&str> {
    match e {
        Expr::Col { name } => Some(name),
        _ => None,
    }
}

impl Builder<'_> {
    fn count_sql(&mut self, r: &Result<(Lowered, KType), Gap>) {
        self.count(r);
    }

    /// A tELTMap (EKOS RFC 0267 phase 2 §ELT maps): its inputs renamed `<alias>__<column>`,
    /// joined in order, then per output a filter, an optional GROUP BY and a projection.
    fn elt_map(&mut self, c: &Component, e: &EltMap) {
        let engine = c.engine.clone();
        let fail = |b: &mut Self, inputs: Vec<NodeId>, reason: String| {
            for o in &e.outputs {
                b.gap(c, &o.name, inputs.clone(), reason.clone());
            }
            if e.outputs.is_empty() {
                b.gap(c, "", inputs, reason);
            }
        };
        if e.inputs.is_empty() {
            return fail(self, Vec::new(), "ELT map without inputs".into());
        }
        let mut prefixed: Vec<NodeId> = Vec::new();
        for input in &e.inputs {
            let link = inbound(self.job, c)
                .into_iter()
                .find(|l| l.label.as_deref() == Some(input.table.as_str()));
            let Some(up) = link.and_then(|l| self.upstream(l)) else {
                return fail(
                    self,
                    Vec::new(),
                    format!("no link brings ELT input `{}`", input.table),
                );
            };
            let alias = elt_alias(input);
            let fields = self.input_schema(up);
            let items = fields
                .iter()
                .map(|f| (qualified(alias, &f.name), Expr::col(&f.name)))
                .collect();
            let schema = fields
                .iter()
                .map(|f| Field {
                    name: qualified(alias, &f.name),
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
                Some(format!("input {alias}")),
            );
            prefixed.push(id);
        }
        let mut core = prefixed[0];
        let mut fidelity = Fidelity::Exact;
        let mut notes: Vec<String> = Vec::new();
        for (i, input) in e.inputs.iter().enumerate().skip(1) {
            match self.elt_join(c, input, core, prefixed[i], engine.as_deref()) {
                Ok((id, f, n)) => {
                    core = id;
                    fidelity = fidelity.max(f);
                    for x in n {
                        if !notes.contains(&x) {
                            notes.push(x);
                        }
                    }
                }
                Err(g) => return fail(self, vec![core, prefixed[i]], g.reason),
            }
        }
        for o in &e.outputs {
            if let Err(g) = self.elt_output(c, o, core, fidelity, &notes, engine.as_deref()) {
                self.gap(c, &o.name, vec![core], g.reason);
            }
        }
    }

    /// One joined ELT input: plain-column equalities are join keys; a key against a literal (or
    /// an expression of the joined input alone) filters that input first; a key against an
    /// expression of the inputs before it is computed on the left first; any other comparison is
    /// a condition, which only an inner join can take.
    fn elt_join(
        &mut self,
        c: &Component,
        input: &EltInput,
        left: NodeId,
        right: NodeId,
        engine: Option<&str>,
    ) -> Result<(NodeId, Fidelity, Vec<String>), Gap> {
        let alias = elt_alias(input);
        let kind = match input.join_type.as_str() {
            "INNER_JOIN" => JoinKind::Inner,
            "LEFT_OUTER_JOIN" => JoinKind::Left,
            "RIGHT_OUTER_JOIN" => JoinKind::Right,
            "FULL_OUTER_JOIN" => JoinKind::Full,
            "CROSS_JOIN" => JoinKind::Cross,
            other => return Err(Gap::new(format!("ELT join type `{other}`"))),
        };
        let left_fields = self.input_schema(left);
        let right_fields = self.input_schema(right);
        let left_cols = combined_lookup(left_fields.clone());
        let right_cols = combined_lookup(right_fields.clone());
        let mut keys: Vec<(String, String)> = Vec::new();
        let mut right_filter: Vec<Expr> = Vec::new();
        let mut left_extra: Vec<(String, Expr, KType)> = Vec::new();
        let mut condition: Vec<Expr> = Vec::new();
        let mut fidelity = Fidelity::Exact;
        let mut notes = Vec::new();
        for (col, op, text) in &input.join_keys {
            let Some((rcol, rty)) = right_cols(alias, col) else {
                return Err(Gap::new(format!(
                    "`{alias}.{col}` is not a column of the ELT input"
                )));
            };
            let parsed = crate::sql::parse(text, engine)?;
            // Which side the expression reads.
            let on_left = crate::sql::lower_ast(&parsed, engine, &left_cols);
            let on_right = crate::sql::lower_ast(&parsed, engine, &right_cols);
            let (lowered, side_left) = match (on_left, on_right) {
                (Ok(l), _) => (l, true),
                (Err(_), Ok(r)) => (r, false),
                (Err(g), Err(_)) => {
                    self.count_sql(&Err(g.clone()));
                    return Err(g);
                }
            };
            self.count_sql(&Ok(lowered.clone()));
            let (l, _) = lowered;
            fidelity = fidelity.max(l.fidelity);
            notes.extend(l.notes.iter().cloned());
            let bin = match op.trim() {
                "=" => BinOp::Eq,
                "<>" | "!=" => BinOp::NotEq,
                "<" => BinOp::Lt,
                "<=" => BinOp::LtEq,
                ">" => BinOp::Gt,
                ">=" => BinOp::GtEq,
                other => return Err(Gap::new(format!("ELT join operator `{other}`"))),
            };
            let constant = l.expr.columns().is_empty();
            if constant || !side_left {
                // A test on the joined input alone: filter it before joining.
                right_filter.push(Expr::bin(bin, rcol, l.expr));
                continue;
            }
            if bin == BinOp::Eq {
                if let Some(name) = plain_column(&l.expr) {
                    keys.push((
                        name.to_string(),
                        plain_column(&rcol).unwrap_or_default().into(),
                    ));
                } else {
                    let name = format!("__join_{alias}_{col}");
                    left_extra.push((name.clone(), l.expr, rty));
                    keys.push((name, plain_column(&rcol).unwrap_or_default().into()));
                }
                continue;
            }
            if kind != JoinKind::Inner {
                return Err(Gap::new(format!(
                    "a `{op}` comparison in an outer join on `{alias}`: only equality keys and tests on the joined input are lowered"
                )));
            }
            condition.push(Expr::bin(bin, l.expr, rcol));
        }
        let mut right = right;
        if !right_filter.is_empty() {
            let first = right_filter.remove(0);
            let p = right_filter.into_iter().fold(first, Expr::and);
            right = self.push(
                c,
                Op::Filter {
                    predicate: Expr::two_valued(p),
                },
                vec![right],
                right_fields.clone(),
                Fidelity::Exact,
                Vec::new(),
                Some(format!("join {alias} (tests on the joined input)")),
            );
        }
        let mut left = left;
        if !left_extra.is_empty() {
            let mut items: Vec<(String, Expr)> = left_fields
                .iter()
                .map(|f| (f.name.clone(), Expr::col(&f.name)))
                .collect();
            let mut schema = left_fields.clone();
            for (n, e, t) in left_extra {
                items.push((n.clone(), e));
                schema.push(Field::new(&n, t));
            }
            left = self.push(
                c,
                Op::Project { items },
                vec![left],
                schema,
                fidelity,
                Vec::new(),
                Some(format!("join {alias} (computed keys)")),
            );
        }
        let mut schema = self.input_schema(left);
        schema.extend(right_fields);
        let cond = (!condition.is_empty()).then(|| {
            let first = condition.remove(0);
            condition.into_iter().fold(first, Expr::and)
        });
        if keys.is_empty() && kind != JoinKind::Cross && cond.is_none() {
            notes.push(format!(
                "ELT input `{alias}` is joined without a key: every row meets every row"
            ));
        }
        let id = self.push(
            c,
            Op::Join {
                kind,
                keys,
                condition: cond,
            },
            vec![left, right],
            schema,
            fidelity,
            notes.clone(),
            Some(format!("join {alias}")),
        );
        Ok((id, fidelity, notes))
    }

    /// One ELT output: filter, GROUP BY (keys and aggregate arguments computed first), ORDER BY,
    /// projection.
    fn elt_output(
        &mut self,
        c: &Component,
        o: &EltOutput,
        core: NodeId,
        fidelity: Fidelity,
        notes: &[String],
        engine: Option<&str>,
    ) -> Result<(), Gap> {
        let mut fidelity = fidelity;
        let mut notes: Vec<String> = notes.to_vec();
        let add = |n: &[String], notes: &mut Vec<String>| {
            for x in n {
                if !notes.contains(x) {
                    notes.push(x.clone());
                }
            }
        };
        let branch = Some(o.name.clone());
        let lookup = combined_lookup(self.input_schema(core));
        let mut head = core;
        // WHERE.
        let mut preds = Vec::new();
        for f in &o.filters {
            let r = crate::sql::lower(f, engine, &lookup);
            self.count_sql(&r);
            let (l, _) = r?;
            fidelity = fidelity.max(l.fidelity);
            add(&l.notes, &mut notes);
            preds.push(Expr::two_valued(l.expr));
        }
        if let Some(first) = preds.first().cloned() {
            let p = preds.into_iter().skip(1).fold(first, Expr::and);
            let s = self.input_schema(core);
            head = self.push(
                c,
                Op::Filter { predicate: p },
                vec![core],
                s,
                fidelity,
                Vec::new(),
                branch.clone(),
            );
        }
        let clauses = crate::sql::parse_clauses(&o.clauses, engine)?;
        let parsed: Vec<(String, sqlparser::ast::Expr, Option<String>)> = o
            .entries
            .iter()
            .map(|en| {
                let text = en.expression.clone().unwrap_or_else(|| "NULL".into());
                crate::sql::parse(&text, engine)
                    .map(|p| (en.name.clone(), p, en.talend_type.clone()))
            })
            .collect::<Result<_, _>>()?;
        let grouped = !clauses.group_by.is_empty()
            || parsed
                .iter()
                .any(|(_, p, _)| crate::sql::aggregate_of(p).is_some());
        let mut items: Vec<(String, Expr)> = Vec::new();
        let mut schema: Vec<Field> = Vec::new();
        if grouped {
            let mut keys: Vec<(Expr, KType)> = Vec::new();
            for g in &clauses.group_by {
                let r = crate::sql::lower_ast(g, engine, &lookup);
                self.count_sql(&r);
                let (l, t) = r?;
                fidelity = fidelity.max(l.fidelity);
                add(&l.notes, &mut notes);
                keys.push((l.expr, t));
            }
            let mut pre: Vec<(String, Expr)> = Vec::new();
            let mut pre_schema: Vec<Field> = Vec::new();
            for (i, (k, t)) in keys.iter().enumerate() {
                pre.push((format!("__g{i}"), k.clone()));
                pre_schema.push(Field::new(&format!("__g{i}"), *t));
            }
            let mut aggs = Vec::new();
            let mut post: Vec<(String, Expr, KType)> = Vec::new();
            for (name, p, ty) in &parsed {
                let declared = talend_type(ty.as_deref());
                // Every aggregate in the expression becomes a column `__agg_<n>` of the
                // Aggregate; what is around it is computed after, over the keys.
                let mut p = p.clone();
                let mut found: Vec<(String, Option<sqlparser::ast::Expr>, bool)> = Vec::new();
                let _ = sqlparser::ast::visit_expressions_mut(&mut p, |e| {
                    if let Some((f, a, d)) = crate::sql::aggregate_of(e) {
                        let a = a.cloned();
                        *e = sqlparser::ast::Expr::Identifier(sqlparser::ast::Ident::new(format!(
                            "__agg_{}",
                            aggs.len() + found.len()
                        )));
                        found.push((f, a, d));
                    }
                    std::ops::ControlFlow::<()>::Continue(())
                });
                for (func, arg, distinct) in found {
                    let n = format!("__agg_{}", aggs.len());
                    let subject = match arg {
                        Some(a) => {
                            let r = crate::sql::lower_ast(&a, engine, &lookup);
                            self.count_sql(&r);
                            let (l, t) = r?;
                            fidelity = fidelity.max(l.fidelity);
                            add(&l.notes, &mut notes);
                            let an = format!("__a{}", aggs.len());
                            pre.push((an.clone(), l.expr));
                            pre_schema.push(Field::new(&an, t));
                            Some((an, t))
                        }
                        None => None,
                    };
                    let func = match (distinct, func.as_str()) {
                        (true, "count") => "count_distinct".to_string(),
                        (true, f) => {
                            return Err(Gap::new(format!("`{f}(DISTINCT …)` in output `{name}`")));
                        }
                        (false, _) => func,
                    };
                    let t = match func.as_str() {
                        "count" | "count_distinct" => KType::Integer,
                        "avg" => KType::Number,
                        _ => subject.as_ref().map_or(KType::Unknown, |(_, t)| *t),
                    };
                    aggs.push(Agg {
                        name: n,
                        func,
                        subject: subject.map(|(n, _)| n),
                        arg: None,
                        ty: t,
                    });
                }
                let agg_types: Vec<(String, KType)> =
                    aggs.iter().map(|a| (a.name.clone(), a.ty)).collect();
                let with_aggs = |q: &str, col: &str| {
                    if q.is_empty()
                        && let Some((n, t)) = agg_types.iter().find(|(n, _)| n == col)
                    {
                        return Some((Expr::col(n), *t));
                    }
                    lookup(q, col)
                };
                let r = crate::sql::lower_ast(&p, engine, &with_aggs);
                self.count_sql(&r);
                let (l, t) = r?;
                fidelity = fidelity.max(l.fidelity);
                add(&l.notes, &mut notes);
                let over_keys = match keys.iter().position(|(k, _)| *k == l.expr) {
                    Some(i) => Expr::col(&format!("__g{i}")),
                    None => {
                        let e = replace_keys(&l.expr, &keys).map_columns(&|col| {
                            if col
                                .strip_prefix("__g")
                                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
                            {
                                return Expr::col(col);
                            }
                            if col.starts_with("__agg_") {
                                return Expr::col(col);
                            }
                            keys.iter()
                                .position(|(k, _)| plain_column(k) == Some(col))
                                .map_or_else(
                                    || Expr::col(&format!("__missing__{col}")),
                                    |i| Expr::col(&format!("__g{i}")),
                                )
                        });
                        if let Some(m) = e
                            .columns()
                            .iter()
                            .find_map(|c| c.strip_prefix("__missing__").map(str::to_string))
                        {
                            return Err(Gap::new(format!(
                                "output column `{name}` reads `{m}`, which is neither grouped nor aggregated"
                            )));
                        }
                        e
                    }
                };
                post.push((
                    name.clone(),
                    over_keys,
                    if declared == KType::Unknown {
                        t
                    } else {
                        declared
                    },
                ));
            }
            let pre_id = self.push(
                c,
                Op::Project { items: pre },
                vec![head],
                pre_schema,
                fidelity,
                Vec::new(),
                branch.clone(),
            );
            let mut agg_schema: Vec<Field> = (0..keys.len())
                .map(|i| Field::new(&format!("__g{i}"), keys[i].1))
                .collect();
            agg_schema.extend(aggs.iter().map(|a| Field::new(&a.name, a.ty)));
            head = self.push(
                c,
                Op::Aggregate {
                    group_by: (0..keys.len()).map(|i| format!("__g{i}")).collect(),
                    aggs,
                },
                vec![pre_id],
                agg_schema,
                fidelity,
                Vec::new(),
                branch.clone(),
            );
            for (n, e, t) in post {
                items.push((n.clone(), e));
                schema.push(Field::new(&n, t));
            }
        } else {
            for (name, p, ty) in &parsed {
                let r = crate::sql::lower_ast(p, engine, &lookup);
                self.count_sql(&r);
                let (l, t) = r?;
                fidelity = fidelity.max(l.fidelity);
                add(&l.notes, &mut notes);
                let declared = talend_type(ty.as_deref());
                items.push((name.clone(), l.expr));
                schema.push(Field::new(
                    name,
                    if declared == KType::Unknown {
                        t
                    } else {
                        declared
                    },
                ));
            }
        }
        if !clauses.order_by.is_empty() {
            let mut keys = Vec::new();
            for ob in &clauses.order_by {
                let (l, _) = crate::sql::lower_ast(&ob.expr, engine, &lookup)?;
                let Some(field) = plain_column(&l.expr) else {
                    return Err(Gap::new(format!("ORDER BY `{}` is not a column", ob.expr)));
                };
                keys.push(SortKey {
                    field: field.to_string(),
                    ascending: ob.asc != Some(false),
                    case_sensitive: true,
                });
            }
            let s = self.input_schema(head);
            head = self.push(
                c,
                Op::Sort { keys },
                vec![head],
                s,
                fidelity,
                Vec::new(),
                branch.clone(),
            );
        }
        let id = self.push(
            c,
            Op::Project { items },
            vec![head],
            schema,
            fidelity,
            notes,
            branch,
        );
        self.set(c, &o.name, id);
        Ok(())
    }
}

/// `e` with every subexpression equal to a GROUP BY key replaced by that key's column
/// (`__g<i>`).
fn replace_keys(e: &Expr, keys: &[(Expr, KType)]) -> Expr {
    if let Some(i) = keys.iter().position(|(k, _)| k == e) {
        return Expr::col(&format!("__g{i}"));
    }
    let r = |x: &Expr| Box::new(replace_keys(x, keys));
    match e {
        Expr::Col { .. } | Expr::Lit { .. } | Expr::Sql { .. } => e.clone(),
        Expr::Unary { op, arg } => Expr::Unary {
            op: *op,
            arg: r(arg),
        },
        Expr::Binary { op, left, right } => Expr::Binary {
            op: *op,
            left: r(left),
            right: r(right),
        },
        Expr::Call { func, args } => Expr::Call {
            func: *func,
            args: args.iter().map(|a| replace_keys(a, keys)).collect(),
        },
        Expr::Case { when, otherwise } => Expr::Case {
            when: when
                .iter()
                .map(|(c, v)| (replace_keys(c, keys), replace_keys(v, keys)))
                .collect(),
            otherwise: otherwise.as_ref().map(|o| r(o)),
        },
        Expr::Cast {
            arg,
            to,
            length,
            precision,
        } => Expr::Cast {
            arg: r(arg),
            to: *to,
            length: *length,
            precision: *precision,
        },
        Expr::IsNull { arg, negated } => Expr::IsNull {
            arg: r(arg),
            negated: *negated,
        },
        Expr::InList { arg, list, negated } => Expr::InList {
            arg: r(arg),
            list: list.iter().map(|a| replace_keys(a, keys)).collect(),
            negated: *negated,
        },
        Expr::Like { arg, pattern } => Expr::Like {
            arg: r(arg),
            pattern: r(pattern),
        },
    }
}

/// A table name as written (`db.schema.table`, `schema.table`, `table`).
fn object_table(name: &sqlparser::ast::ObjectName) -> TableRef {
    let parts: Vec<String> = name.0.iter().map(|i| i.value.clone()).collect();
    let (schema, table) = match parts.as_slice() {
        [t] => (None, t.clone()),
        [.., s, t] => (Some(s.clone()), t.clone()),
        [] => (None, String::new()),
    };
    TableRef {
        connection: None,
        schema,
        table,
    }
}

impl Builder<'_> {
    /// A `tDBRow` / `tXRow` (EKOS RFC 0267 phase 2 §Statements): one SQL statement run from a
    /// trigger.
    fn statement(&mut self, c: &Component) {
        use sqlparser::ast::{AssignmentTarget, Statement};
        let Some(q) = param(c, "QUERY") else {
            self.gap(
                c,
                "",
                Vec::new(),
                "statement component without a query".into(),
            );
            return;
        };
        let folded = self.job.fold(q);
        if !folded.complete() {
            self.gap(
                c,
                "",
                Vec::new(),
                format!(
                    "the statement depends on values known only at run time: {}",
                    folded.holes.join(", ")
                ),
            );
            return;
        }
        let text = folded.text.trim().trim_end_matches(';').trim().to_string();
        let engine = c.engine.clone().or_else(|| {
            c.component_type
                .contains("Snowflake")
                .then(|| "snowflake".to_string())
        });
        let d = crate::sql::dialect(engine.as_deref());
        let stmts = match sqlparser::parser::Parser::parse_sql(d.as_ref(), &text) {
            Ok(s) => s,
            Err(e) => {
                self.gap(
                    c,
                    "",
                    Vec::new(),
                    format!("the statement does not parse: {e}"),
                );
                return;
            }
        };
        let [stmt] = stmts.as_slice() else {
            self.gap(
                c,
                "",
                Vec::new(),
                format!("{} statements in one component", stmts.len()),
            );
            return;
        };
        let mut tables: Vec<TableRef> = Vec::new();
        let _ = sqlparser::ast::visit_relations(stmt, |r| {
            let t = object_table(r);
            if !tables.contains(&t) {
                tables.push(t);
            }
            std::ops::ControlFlow::<()>::Continue(())
        });
        let verb = match stmt {
            Statement::Update { .. } => "UPDATE",
            Statement::Delete(_) => "DELETE",
            Statement::Insert(_) => "INSERT",
            Statement::Merge { .. } => "MERGE",
            Statement::Truncate { .. } => "TRUNCATE",
            other => {
                let first = other.to_string();
                let word = first
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                self.gap(
                    c,
                    "",
                    Vec::new(),
                    format!("a `{word}` statement (only UPDATE, DELETE, INSERT, MERGE and TRUNCATE are lowered)"),
                );
                return;
            }
        };
        let target = tables.first().cloned();
        let reads: Vec<TableRef> = tables.iter().skip(1).cloned().collect();
        // A statement's columns are the changed table's, unqualified or qualified by its name.
        let target_name = target.as_ref().map(|t| t.table.clone()).unwrap_or_default();
        let column = move |q: &str, col: &str| {
            (q.is_empty() || q.eq_ignore_ascii_case(&target_name))
                .then(|| (Expr::col(col), KType::Unknown))
        };
        let mut set = Vec::new();
        let mut predicate = None;
        let mut fidelity = Fidelity::Rewritten;
        let mut notes = vec![format!(
            "a {verb} statement run from a trigger, not on rows: RFC 0268 places it (a hook)"
        )];
        notes.extend(self.run_time_notes(q));
        let mut lower = |b: &mut Self, e: &sqlparser::ast::Expr| -> Result<Expr, Gap> {
            let r = crate::sql::lower_ast(e, engine.as_deref(), &column);
            b.count_sql(&r);
            let (l, _) = r?;
            fidelity = fidelity.max(l.fidelity);
            for n in l.notes {
                if !notes.contains(&n) {
                    notes.push(n);
                }
            }
            Ok(l.expr)
        };
        let lowered: Result<(), Gap> = (|| {
            match stmt {
                Statement::Update {
                    assignments,
                    selection,
                    from: None,
                    ..
                } => {
                    for a in assignments {
                        let AssignmentTarget::ColumnName(n) = &a.target else {
                            return Err(Gap::new("UPDATE of a tuple of columns"));
                        };
                        let col = n.0.last().map(|i| i.value.clone()).unwrap_or_default();
                        set.push((col, lower(self, &a.value)?));
                    }
                    if let Some(w) = selection {
                        predicate = Some(lower(self, w)?);
                    }
                }
                Statement::Update { .. } => {
                    return Err(Gap::new("UPDATE … FROM (a join in an update)"));
                }
                Statement::Delete(d) => {
                    if let Some(w) = &d.selection {
                        predicate = Some(lower(self, w)?);
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(g) = lowered {
            self.gap(c, "", Vec::new(), g.reason);
            return;
        }
        let id = self.push(
            c,
            Op::Statement {
                verb: verb.into(),
                target,
                reads,
                set,
                predicate,
                sql: text,
            },
            Vec::new(),
            Vec::new(),
            fidelity,
            notes,
            None,
        );
        self.set(c, "", id);
    }
}

/// `context.<var> = input_row.<column>;` in every `tJavaRow` of the job.
fn context_writers(job: &Job) -> BTreeMap<String, (String, String)> {
    use ekos_lakehouse_etl_expr::java::JExpr;
    use ekos_lakehouse_etl_expr::stmt::{Stmt, parse_script};
    let mut out = BTreeMap::new();
    for c in job
        .components
        .iter()
        .filter(|c| c.component_type == "tJavaRow")
    {
        let Some(code) = param(c, "CODE") else {
            continue;
        };
        let Ok(stmts) = parse_script(code, false) else {
            continue;
        };
        for st in stmts {
            if let Stmt::Expr(JExpr::Assign("=", target, value)) = st
                && let JExpr::Field(q, var) = target.as_ref()
                && matches!(q.as_ref(), JExpr::Name(n) if n == "context")
                && let JExpr::Field(r, col) = value.as_ref()
                && matches!(r.as_ref(), JExpr::Name(n) if n == "input_row")
            {
                out.insert(var.clone(), (c.unique_name.clone(), col.clone()));
            }
        }
    }
    out
}

/// `NB_ROWS` naming `context.<v>` that a `tJava` assigns (EKOS RFC 0268 phase 2): `None` when no
/// `tJava` does; the count with its note for `TalendDate.diffDate(context.E, context.S, "dd")`
/// (optionally cast, plus a constant), computed from the two defaults; a gap otherwise.
fn set_by_tjava(job: &Job, nb_rows: &str) -> Option<Result<(u64, String), String>> {
    use ekos_lakehouse_etl_expr::java::JExpr;
    use ekos_lakehouse_etl_expr::stmt::{Stmt, parse_script};
    let var = nb_rows.trim().strip_prefix("context.")?;
    if !var.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
        return None;
    }
    let assigns = |code: &str| {
        let compact: String = code.chars().filter(|ch| !ch.is_whitespace()).collect();
        compact.contains(&format!("context.{var}="))
    };
    let tjava = job
        .components
        .iter()
        .find(|c| c.component_type == "tJava" && param(c, "CODE").is_some_and(assigns))?;
    let who = tjava.unique_name.clone();
    let unknown = || {
        Err(format!(
            "the row count `context.{var}` is set at run time by {who}, in a form the lowering does not compute"
        ))
    };
    let Ok(stmts) = parse_script(param(tjava, "CODE").unwrap_or_default(), false) else {
        return Some(unknown());
    };
    let value = stmts.iter().find_map(|st| match st {
        Stmt::Expr(JExpr::Assign("=", target, value))
            if matches!(target.as_ref(), JExpr::Field(q, v)
                if v == var && matches!(q.as_ref(), JExpr::Name(n) if n == "context")) =>
        {
            Some(value.as_ref().clone())
        }
        _ => None,
    });
    let Some(value) = value else {
        return Some(unknown());
    };
    let (call, plus) = match &value {
        JExpr::Binary("+", l, r) => match r.as_ref() {
            JExpr::Int(k) => (l.as_ref().clone(), *k),
            _ => return Some(unknown()),
        },
        other => (other.clone(), 0),
    };
    let call = match call {
        JExpr::Cast(_, inner) => *inner,
        other => other,
    };
    let ctx = |e: &JExpr| -> Option<String> {
        match e {
            JExpr::Field(q, v) if matches!(q.as_ref(), JExpr::Name(n) if n == "context") => {
                Some(v.clone())
            }
            _ => None,
        }
    };
    let JExpr::Call(Some(class), name, args) = &call else {
        return Some(unknown());
    };
    if !matches!(class.as_ref(), JExpr::Name(n) if n == "TalendDate")
        || name != "diffDate"
        || args.len() != 3
        || !matches!(&args[2], JExpr::Str(p) if p == "dd")
    {
        return Some(unknown());
    }
    let (Some(end), Some(start)) = (ctx(&args[0]), ctx(&args[1])) else {
        return Some(unknown());
    };
    let day = |v: &str| {
        job.context_var(v)
            .and_then(|c| c.value.clone())
            .and_then(|t| days_from_civil(&t))
    };
    let (Some(e), Some(s)) = (day(&end), day(&start)) else {
        return Some(Err(format!(
            "the row count `context.{var}` is set by {who} from `context.{end}` and `context.{start}`, which have no date default"
        )));
    };
    let n = e - s + plus;
    let Ok(n) = u64::try_from(n) else {
        return Some(unknown());
    };
    Some(Ok((
        n,
        format!(
            "the row count `context.{var}` is set at run time by {who}: the days from `context.{start}` to `context.{end}`{}, computed from their default values (replaceable at run time)",
            if plus == 0 {
                String::new()
            } else {
                format!(" plus {plus}")
            }
        ),
    )))
}

/// Days since 1970-01-01 of a `yyyy-MM-dd…` text (Howard Hinnant's civil-days algorithm).
fn days_from_civil(text: &str) -> Option<i64> {
    let d = text.trim().get(..10)?;
    let mut it = d.split('-');
    let (y, m, day): (i64, i64, i64) = (
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// `System.out.println(…)` / `print`, and an `if` holding nothing else: logging, not data.
fn is_logging(st: &ekos_lakehouse_etl_expr::stmt::Stmt) -> bool {
    use ekos_lakehouse_etl_expr::java::JExpr;
    use ekos_lakehouse_etl_expr::stmt::Stmt;
    match st {
        Stmt::Expr(JExpr::Call(Some(recv), name, _)) => {
            matches!(name.as_str(), "println" | "print")
                && matches!(recv.as_ref(), JExpr::Field(s, out)
                    if (out == "out" || out == "err") && matches!(s.as_ref(), JExpr::Name(n) if n == "System"))
        }
        Stmt::If(_, then, other) => then.iter().chain(other).all(is_logging),
        Stmt::Block(b) => b.iter().all(is_logging),
        _ => false,
    }
}

impl Builder<'_> {
    /// Notes for the context variables `text` reads that are known only at run time: set from a
    /// row by a `tJavaRow` (a watermark), or replaceable by an implicit context load or a
    /// `tContextLoad` (RFC 0264).
    fn run_time_notes(&self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (_, var) in ekos_talend::qualified_refs(text, &["context"]) {
            if let Some((component, column)) = self.writers.get(&var) {
                out.push(format!(
                    "`context.{var}` is set at run time by {component} from column `{column}` (a watermark): the default value is not what a run reads"
                ));
            } else if self.runtime.overridable(&var) {
                out.push(format!(
                    "`context.{var}` can be replaced at run time (implicit context load or tContextLoad): lowered to its default value"
                ));
            }
        }
        out
    }

    /// A `tJavaRow` (EKOS RFC 0267 phase 2): `output_row.<c> = <expr>;` is a projection,
    /// `context.<v> = input_row.<c>;` a run-time context value, logging is dropped; anything
    /// else is a gap.
    fn java_row(&mut self, c: &Component) {
        use ekos_lakehouse_etl_expr::java::JExpr;
        use ekos_lakehouse_etl_expr::stmt::{Stmt, parse_script};
        let input = self.main_input(c);
        let inputs: Vec<NodeId> = input.into_iter().collect();
        let Some(code) = param(c, "CODE") else {
            self.gap(c, "", inputs, "tJavaRow without code".into());
            return;
        };
        let stmts = match parse_script(code, false) {
            Ok(s) => s,
            Err(g) => {
                self.gap(c, "", inputs, format!("tJavaRow code: {}", g.reason));
                return;
            }
        };
        let in_fields = input.map(|i| self.input_schema(i)).unwrap_or_default();
        let column = {
            let f = in_fields.clone();
            move |q: &str, col: &str| {
                (q == "input_row")
                    .then(|| f.iter().find(|x| x.name == col))
                    .flatten()
                    .map(|x| (Expr::col(&x.name), x.ty))
            }
        };
        let context = context_of(self.job);
        let global = global_of(self.job);
        let none = |_: &str| None;
        let scope = Scope {
            column: &column,
            var: &none,
            context: &context,
            job: &self.job.name,
            global: &global,
            routines: self.routines,
            locale: self.options.locale,
            sequence: &no_sequence,
        };
        let mut assigned: Vec<(String, Expr, KType)> = Vec::new();
        let mut fidelity = Fidelity::Exact;
        let mut notes: Vec<String> = Vec::new();
        let mut failure: Option<String> = None;
        let mut results = Vec::new();
        for st in &stmts {
            if is_logging(st) {
                let n = "the code's printing (System.out) is dropped: it is logging, not data"
                    .to_string();
                if !notes.contains(&n) {
                    notes.push(n);
                }
                continue;
            }
            let Stmt::Expr(JExpr::Assign("=", target, value)) = st else {
                failure.get_or_insert(format!("tJavaRow statement not translated: {st:?}"));
                continue;
            };
            let JExpr::Field(q, name) = target.as_ref() else {
                failure.get_or_insert(format!("tJavaRow assignment to {target:?}"));
                continue;
            };
            match q.as_ref() {
                JExpr::Name(n) if n == "output_row" => {
                    let r = crate::expr::lower_parsed(value, &scope);
                    results.push(r.as_ref().map(|_| ()).map_err(Clone::clone));
                    match r {
                        Ok((l, t)) => {
                            fidelity = fidelity.max(l.fidelity);
                            for x in l.notes {
                                if !notes.contains(&x) {
                                    notes.push(x);
                                }
                            }
                            assigned.retain(|(n, _, _)| n != name);
                            assigned.push((name.clone(), l.expr, t));
                        }
                        Err(g) => {
                            failure.get_or_insert(format!("`{name}`: {}", g.reason));
                        }
                    }
                }
                JExpr::Name(n) if n == "context" => {
                    let JExpr::Field(r, col) = value.as_ref() else {
                        failure.get_or_insert(format!(
                            "`context.{name}` is set from a computed value"
                        ));
                        continue;
                    };
                    if !matches!(r.as_ref(), JExpr::Name(n) if n == "input_row") {
                        failure.get_or_insert(format!("`context.{name}` is set from {r:?}"));
                        continue;
                    }
                    self.out.context_from_data.push(ContextFromData {
                        var: name.clone(),
                        component: c.unique_name.clone(),
                        column: col.clone(),
                        from: input,
                    });
                }
                other => {
                    failure.get_or_insert(format!("tJavaRow assignment to {other:?}.{name}"));
                }
            }
        }
        for r in results {
            self.count_unit(r);
        }
        if let Some(reason) = failure {
            self.gap(c, "", inputs, reason);
            return;
        }
        let Some(input) = input else {
            self.gap(c, "", Vec::new(), "tJavaRow with no row link in".into());
            return;
        };
        let out_fields = schema(c, "");
        let mut items = Vec::new();
        let mut fields = Vec::new();
        let mut unassigned = Vec::new();
        for f in &out_fields {
            match assigned.iter().find(|(n, _, _)| *n == f.name) {
                Some((_, e, t)) => {
                    items.push((f.name.clone(), e.clone()));
                    fields.push(Field {
                        ty: if f.ty == KType::Unknown { *t } else { f.ty },
                        ..f.clone()
                    });
                }
                None => {
                    unassigned.push(f.name.clone());
                    items.push((f.name.clone(), Expr::null()));
                    fields.push(f.clone());
                }
            }
        }
        if !unassigned.is_empty() && !assigned.is_empty() {
            notes.push(format!(
                "the code does not assign {}: null, as in Talend",
                unassigned.join(", ")
            ));
        }
        if assigned.is_empty()
            && !self
                .out
                .context_from_data
                .iter()
                .any(|x| x.component == c.unique_name)
        {
            notes.push("the code assigns no output column".into());
        }
        let id = self.push(
            c,
            Op::Project { items },
            vec![input],
            fields,
            fidelity,
            notes,
            None,
        );
        self.set(c, "", id);
    }

    /// A `tSampleRow`: the rows at the listed line numbers (`1,5`, `10..20`), by arrival order.
    fn sample_row(&mut self, c: &Component) {
        let Some(input) = self.main_input(c) else {
            self.gap(c, "", Vec::new(), "tSampleRow with no row link in".into());
            return;
        };
        let range = param(c, "RANGE").map(|r| self.job.fold(r));
        let Some(range) = range.filter(|f| f.complete()).map(|f| f.text) else {
            self.gap(
                c,
                "",
                vec![input],
                "tSampleRow range known only at run time".into(),
            );
            return;
        };
        let line = Expr::col(DUP_ROW);
        let mut parts = Vec::new();
        for item in range.split(',').map(str::trim).filter(|x| !x.is_empty()) {
            let num = |t: &str| t.trim().parse::<i64>().ok();
            let p = match item.split_once("..") {
                Some((a, b)) => match (num(a), num(b)) {
                    (Some(a), Some(b)) => Expr::and(
                        Expr::bin(BinOp::GtEq, line.clone(), Expr::int(a)),
                        Expr::bin(BinOp::LtEq, line.clone(), Expr::int(b)),
                    ),
                    _ => {
                        self.gap(c, "", vec![input], format!("tSampleRow range `{range}`"));
                        return;
                    }
                },
                None => match num(item) {
                    Some(n) => Expr::bin(BinOp::Eq, line.clone(), Expr::int(n)),
                    None => {
                        self.gap(c, "", vec![input], format!("tSampleRow range `{range}`"));
                        return;
                    }
                },
            };
            parts.push(p);
        }
        let Some(first) = parts.first().cloned() else {
            self.gap(c, "", vec![input], "tSampleRow with an empty range".into());
            return;
        };
        let predicate = parts.into_iter().skip(1).fold(first, Expr::or);
        let schema = self.input_schema(input);
        let mut numbered = schema.clone();
        numbered.push(Field::new(DUP_ROW, KType::Integer));
        let note = "line numbers count rows in arrival order, which SQL does not keep: the order must be stated".to_string();
        let w = self.push(
            c,
            Op::Window {
                partition: Vec::new(),
                order: Vec::new(),
                items: vec![(
                    DUP_ROW.to_string(),
                    ekos_lakehouse_etl_rel::WindowFn::Sequence {
                        start: 1,
                        increment: 1,
                    },
                )],
            },
            vec![input],
            numbered.clone(),
            Fidelity::Rewritten,
            vec![note.clone()],
            None,
        );
        let f = self.push(
            c,
            Op::Filter { predicate },
            vec![w],
            numbered,
            Fidelity::Rewritten,
            Vec::new(),
            None,
        );
        let items = schema
            .iter()
            .map(|x| (x.name.clone(), Expr::col(&x.name)))
            .collect();
        let id = self.push(
            c,
            Op::Project { items },
            vec![f],
            schema,
            Fidelity::Rewritten,
            vec![note],
            None,
        );
        self.set(c, "", id);
    }
}

/// The name an ELT map's expressions use for an input.
fn elt_alias(i: &EltInput) -> &str {
    if i.alias.is_empty() {
        &i.name
    } else {
        &i.alias
    }
}

/// No `Numeric.sequence` column where a component computes none.
fn no_sequence(_: &str) -> Option<Expr> {
    None
}

/// Every `Numeric.sequence` name in the job's tMaps, with where it is read: `(component, output)`,
/// or `(component, "")` for a variable.
fn sequence_users(job: &Job) -> BTreeMap<String, Vec<(String, String)>> {
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for c in &job.components {
        let mut add = |text: &str, port: &str| {
            for (name, _) in crate::expr::sequences(text) {
                out.entry(name)
                    .or_default()
                    .push((c.unique_name.clone(), port.to_string()));
            }
        };
        if matches!(
            c.component_type.as_str(),
            "tRowGenerator" | "tFixedFlowInput"
        ) {
            for table in ["VALUES"] {
                for r in rows(c, table) {
                    for key in ["ARRAY", "VALUE"] {
                        if let Some(t) = r.get(key) {
                            add(t, "");
                        }
                    }
                }
            }
            continue;
        }
        let Some(m) = &c.mapper else {
            continue;
        };
        for v in &m.vars {
            add(v.expression.as_deref().unwrap_or_default(), "");
        }
        for o in &m.outputs {
            if let Some(f) = &o.filter {
                add(f, &o.name);
            }
            for e in &o.entries {
                add(e.expression.as_deref().unwrap_or_default(), &o.name);
            }
        }
    }
    out
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
                // Java's `File.getName()`: after the last separator of either kind. A job run on
                // Windows stores `C:\dir\file.csv` (found by the differential run, RFC 0268 phase 3).
                Expr::call(
                    ekos_lakehouse_etl_expr::Func::RegexpReplace,
                    vec![path, Expr::str("^.*[/\\\\]"), Expr::str("")],
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

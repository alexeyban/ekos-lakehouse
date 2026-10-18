//! One lowering per step type (RFC 0202). Each reads the step's typed configuration from its
//! XML tree and returns an IR operation, or a gap naming what stopped it.

use crate::build::{Builder, Input, Lowering};
use crate::model::{
    Agg, Field, JoinKind, LookupValue, NodeId, Op, Scd, Sink, SinkMode, SortKey, SourceQuery,
    TableRef, WindowFn,
};
use crate::source;
use ekos_lakehouse_kettle::variables::Scope;
use ekos_lakehouse_kettle::xml::Node;
use ekos_lakehouse_kettle::{Step, Transformation};
use ekos_lakehouse_kettle_expr::calculator::{self, Calculation};
use ekos_lakehouse_kettle_expr::condition::{self, literal};
use ekos_lakehouse_kettle_expr::semantics::kettle_compare;
use ekos_lakehouse_kettle_expr::{
    BinOp, Expr, Fidelity, Func, Gap, KType, Lowered, formula, infer, java,
};
use std::collections::BTreeMap;

/// A Project's output: `(name, expression)` in order.
type Items = Vec<(String, Expr)>;

/// Accumulates a step's label and notes while it is lowered.
struct Acc {
    fidelity: Fidelity,
    notes: Vec<String>,
}

impl Acc {
    fn new() -> Acc {
        Acc {
            fidelity: Fidelity::Exact,
            notes: Vec::new(),
        }
    }
    fn absorb(&mut self, l: &Lowered) {
        self.fidelity = self.fidelity.max(l.fidelity);
        for n in &l.notes {
            self.note_only(n);
        }
    }
    fn note(&mut self, n: impl Into<String>) {
        self.fidelity = self.fidelity.max(Fidelity::Rewritten);
        self.note_only(&n.into());
    }
    fn note_only(&mut self, n: &str) {
        if !self.notes.iter().any(|x| x == n) {
            self.notes.push(n.to_string());
        }
    }
}

fn type_of(schema: &[Field]) -> impl Fn(&str) -> KType + '_ {
    move |n: &str| {
        schema
            .iter()
            .find(|f| f.name == n)
            .map(|f| f.ty)
            .unwrap_or(KType::Unknown)
    }
}

fn has(schema: &[Field], name: &str) -> bool {
    schema.iter().any(|f| f.name == name)
}

fn need(schema: &[Field], name: &str, what: &str) -> Result<(), Gap> {
    if has(schema, name) {
        Ok(())
    } else {
        Err(Gap::new(format!(
            "{what} `{name}` is not in the step's input"
        )))
    }
}

fn int_at(n: &Node, path: &str, default: i32) -> i32 {
    n.text_at(path)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn passthrough(schema: &[Field]) -> Vec<(String, Expr)> {
    schema
        .iter()
        .map(|f| (f.name.clone(), Expr::col(&f.name)))
        .collect()
}

/// A Project node's output: input fields, then `added`, where a later item may read an earlier
/// one (it is substituted by its expression, so the node stays one SELECT).
struct Projection {
    items: Vec<(String, Expr)>,
    schema: Vec<Field>,
}

impl Projection {
    fn over(schema: &[Field]) -> Projection {
        Projection {
            items: passthrough(schema),
            schema: schema.to_vec(),
        }
    }

    /// Substitutes earlier computed fields into `e`.
    fn inline(&self, e: &Expr) -> Expr {
        e.map_columns(&|c| {
            self.items
                .iter()
                .rev()
                .find(|(n, _)| n == c)
                .map(|(_, x)| x.clone())
                .unwrap_or_else(|| Expr::col(c))
        })
    }

    fn set(&mut self, name: &str, e: Expr, f: Field) {
        let e = self.inline(&e);
        match self.items.iter().position(|(n, _)| n == name) {
            Some(i) => {
                self.items[i].1 = e;
                self.schema[i] = f;
            }
            None => {
                self.items.push((name.to_string(), e));
                self.schema.push(f);
            }
        }
    }

    fn remove(&mut self, name: &str) {
        if let Some(i) = self.items.iter().position(|(n, _)| n == name) {
            self.items.remove(i);
            self.schema.remove(i);
        }
    }

    fn ty(&self) -> impl Fn(&str) -> KType + '_ {
        type_of(&self.schema)
    }
}

/// The output fields a code step declares (RFC 0205 asks a model for exactly these).
pub fn declared_outputs(step: &Step) -> Vec<Field> {
    let x = &step.xml;
    let fields = x
        .child("fields")
        .map(|f| f.children_named("field").collect::<Vec<_>>())
        .unwrap_or_default();
    match step.type_id.as_str() {
        "ScriptValueMod" => fields
            .iter()
            .map(|f| {
                let name = f
                    .text_at("rename")
                    .or(f.text_at("name"))
                    .unwrap_or_default();
                Field::new(
                    name,
                    f.text_at("type").map(KType::parse).unwrap_or(KType::String),
                )
            })
            .collect(),
        "UserDefinedJavaClass" => fields
            .iter()
            .map(|f| {
                Field::new(
                    f.text_at("field_name").unwrap_or_default(),
                    f.text_at("field_type")
                        .map(KType::parse)
                        .unwrap_or(KType::String),
                )
            })
            .collect(),
        "Janino" | "Formula" => x
            .children_named("formula")
            .map(|f| {
                let name = f
                    .text_at("replace_field")
                    .or(f.text_at("field_name"))
                    .unwrap_or_default();
                Field::new(
                    name,
                    f.text_at("value_type")
                        .map(KType::parse)
                        .unwrap_or(KType::String),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Kettle's value conversion from `from` to `to` (Select values' meta tab, retyping).
pub fn convert(e: Expr, from: KType, to: KType, mask: Option<&str>) -> Result<Lowered, Gap> {
    use KType::*;
    if from == to || to == Unknown {
        return Ok(Lowered::exact(e));
    }
    let date_mask = mask.unwrap_or(condition::DEFAULT_DATE_MASK);
    Ok(match (from, to) {
        (String, Date | Timestamp) => {
            let l = Expr::call(Func::ParseDatetime, vec![e, Expr::str(date_mask)]);
            if mask.is_some() {
                Lowered::exact(l)
            } else {
                Lowered::rewritten(
                    l,
                    format!("no conversion mask: Kettle's default `{date_mask}` assumed"),
                )
            }
        }
        (Date | Timestamp, String) => {
            let l = Expr::call(Func::FormatDatetime, vec![e, Expr::str(date_mask)]);
            if mask.is_some() {
                Lowered::exact(l)
            } else {
                Lowered::rewritten(
                    l,
                    format!("no conversion mask: Kettle's default `{date_mask}` assumed"),
                )
            }
        }
        (String, Integer | Number | BigNumber) => {
            let l = Expr::cast(Expr::call(Func::Trim, vec![e]), to);
            match mask {
                Some(m) if m.contains([',', '¤', '%']) => {
                    return Err(Gap::new(format!(
                        "number conversion mask `{m}` (grouping, currency or percent)"
                    )));
                }
                Some(m) => Lowered::rewritten(
                    l,
                    format!(
                        "conversion mask `{m}` not applied; the text is read as a plain number"
                    ),
                ),
                None => Lowered::exact(l),
            }
        }
        (Integer, String) => Lowered::exact(Expr::cast(e, String)),
        (Number | BigNumber, String) => Lowered::rewritten(
            Expr::cast(e, String),
            "number to text: Kettle formats with the field's mask; the SQL prints the plain value",
        ),
        (Integer | BigNumber, Number) | (Integer, BigNumber) => Lowered::exact(Expr::cast(e, to)),
        (Number | BigNumber, Integer) => Lowered::rewritten(
            Expr::cast(e, Integer),
            "number to integer: Kettle rounds half up, Trino's CAST rounds half away from zero",
        ),
        (Number, BigNumber) => Lowered::exact(Expr::cast(e, to)),
        (Boolean, String) => {
            Lowered::exact(Expr::case(vec![(e, Expr::str("Y"))], Some(Expr::str("N"))))
        }
        (String, Boolean) => Lowered::rewritten(
            Expr::InList {
                arg: Box::new(Expr::call(
                    Func::Upper,
                    vec![Expr::call(Func::Trim, vec![e])],
                )),
                list: ["Y", "YES", "TRUE", "1"]
                    .iter()
                    .map(|s| Expr::str(s))
                    .collect(),
                negated: false,
            },
            "text to boolean: Y, YES, TRUE and 1 are true",
        ),
        (Unknown, t) => Lowered::rewritten(
            Expr::cast(e, t),
            "the source type is not known; a plain CAST is used",
        ),
        (f, t) => return Err(Gap::new(format!("conversion from {f:?} to {t:?}"))),
    })
}

impl Builder<'_> {
    pub(crate) fn lower_step(
        &mut self,
        t: &Transformation,
        step: &Step,
        prefix: &str,
        scope: &Scope,
        inputs: &[Input],
    ) -> Result<Lowering, Gap> {
        let x = &step.xml;
        match step.type_id.as_str() {
            "TableInput" => self.table_input(t, step, scope, inputs),
            "DataGrid" => data_grid(x),
            "RowGenerator" => row_generator(x),
            "Dummy" | "MappingOutput" | "BlockingStep" | "BlockUntilStepsFinish" => {
                let input = self.single_input(step, prefix, inputs)?;
                let schema = self.ir.nodes[input].schema.clone();
                Ok(Lowering::Node(
                    Op::Project {
                        items: passthrough(&schema),
                    },
                    vec![input],
                    schema,
                    Fidelity::Exact,
                    vec![],
                ))
            }
            "MappingInput" => {
                let Some(i) = inputs.iter().find(|i| i.from == "mapping input") else {
                    return Err(Gap::new("a Mapping input step outside an inlined Mapping"));
                };
                let schema = self.ir.nodes[i.node].schema.clone();
                Ok(Lowering::Node(
                    Op::Project {
                        items: passthrough(&schema),
                    },
                    vec![i.node],
                    schema,
                    Fidelity::Exact,
                    vec![],
                ))
            }
            "Mapping" | "SimpleMapping" => self.mapping(t, step, prefix, inputs),
            "Abort" => {
                let input = self.single_input(step, prefix, inputs)?;
                let message = x
                    .text_at("message")
                    .unwrap_or("Abort step reached")
                    .to_string();
                Ok(Lowering::Node(
                    Op::Assert { message },
                    vec![input],
                    vec![],
                    Fidelity::Exact,
                    vec![],
                ))
            }
            "Append" => self.append(x, inputs),
            "MergeJoin" => self.merge_join(x, inputs),
            "JoinRows" => self.join_rows(x, inputs),
            "StreamLookup" => self.stream_lookup(x, inputs),
            "DBLookup" => {
                let input = self.single_input(step, prefix, inputs)?;
                self.db_lookup(t, x, input)
            }
            "ScriptValueMod" | "UserDefinedJavaClass" => {
                let input = self.single_input(step, prefix, inputs)?;
                self.code_step(step, prefix, input, None)
            }
            _ if step.category == ekos_lakehouse_kettle::Category::Unknown
                && self.plugin_code.contains_key(&step.type_id) =>
            {
                let input = self.single_input(step, prefix, inputs)?;
                let src = self.plugin_code[&step.type_id].clone();
                self.code_step(step, prefix, input, Some(src))
            }
            "TableOutput" | "InsertUpdate" | "Update" | "Delete" => {
                let input = self.single_input(step, prefix, inputs)?;
                self.sink(t, step, input)
            }
            "DimensionLookup" => {
                let input = self.single_input(step, prefix, inputs)?;
                self.dimension(t, x, input)
            }
            _ => {
                let input = self.single_input(step, prefix, inputs)?;
                let schema = self.ir.nodes[input].schema.clone();
                self.one_input(step, scope, input, &schema)
            }
        }
    }

    /// Steps with one input whose output is a function of it.
    fn one_input(
        &mut self,
        step: &Step,
        scope: &Scope,
        input: NodeId,
        schema: &[Field],
    ) -> Result<Lowering, Gap> {
        let x = &step.xml;
        let node = |op, schema: Vec<Field>, acc: Acc| {
            Ok(Lowering::Node(
                op,
                vec![input],
                schema,
                acc.fidelity,
                acc.notes,
            ))
        };
        let mut acc = Acc::new();
        match step.type_id.as_str() {
            "SelectValues" => {
                let (items, out) = select_values(x, schema, &mut acc)?;
                node(Op::Project { items }, out, acc)
            }
            "Calculator" => {
                let mut p = Projection::over(schema);
                let mut removed = Vec::new();
                for c in x.children_named("calculation") {
                    let calc = Calculation {
                        field_name: c.text_at("field_name").unwrap_or_default().to_string(),
                        calc_type: c.text_at("calc_type").unwrap_or_default().to_string(),
                        field_a: c.text_at("field_a").map(str::to_string),
                        field_b: c.text_at("field_b").map(str::to_string),
                        field_c: c.text_at("field_c").map(str::to_string),
                        value_type: c
                            .text_at("value_type")
                            .map(KType::parse)
                            .unwrap_or_default(),
                        length: int_at(c, "value_length", -1),
                        precision: int_at(c, "value_precision", -1),
                        conversion_mask: c.text_at("conversion_mask").map(str::to_string),
                    };
                    for f in [&calc.field_a, &calc.field_b, &calc.field_c]
                        .into_iter()
                        .flatten()
                    {
                        if calc.calc_type != "CONSTANT" && !f.is_empty() {
                            need(&p.schema, f, "field")?;
                        }
                    }
                    let l = calculator::lower(&calc, &p.ty())
                        .map_err(|g| Gap::new(format!("{}: {g}", calc.field_name)))?;
                    acc.absorb(&l);
                    let ty = if calc.value_type == KType::Unknown {
                        infer(&l.expr, &p.ty())
                    } else {
                        calc.value_type
                    };
                    p.set(
                        &calc.field_name,
                        l.expr,
                        Field {
                            name: calc.field_name.clone(),
                            ty,
                            length: calc.length,
                            precision: calc.precision,
                        },
                    );
                    if c.flag_at("remove", false) {
                        removed.push(calc.field_name.clone());
                    }
                }
                for r in removed {
                    p.remove(&r);
                }
                node(Op::Project { items: p.items }, p.schema, acc)
            }
            "Formula" | "Janino" => {
                let mut p = Projection::over(schema);
                for f in x.children_named("formula") {
                    let name = f.text_at("field_name").unwrap_or_default().to_string();
                    let src = f.raw_at("formula_string").unwrap_or_default();
                    let vt = f
                        .text_at("value_type")
                        .map(KType::parse)
                        .unwrap_or_default();
                    let (l, inferred) = if step.type_id == "Formula" {
                        let l = formula::lower(src, &p.ty())
                            .map_err(|g| Gap::new(format!("{name}: {g}")))?;
                        let t = infer(&l.expr, &p.ty());
                        (l, t)
                    } else {
                        let ft = |n: &str| p.schema.iter().find(|f| f.name == n).map(|f| f.ty);
                        java::lower_over_fields(src, &ft)
                            .map_err(|g| Gap::new(format!("{name}: {g}")))?
                    };
                    acc.absorb(&l);
                    let target = f.text_at("replace_field").unwrap_or(&name).to_string();
                    let ty = if vt == KType::Unknown { inferred } else { vt };
                    let e = if vt != KType::Unknown && inferred != vt {
                        Expr::Cast {
                            arg: Box::new(l.expr),
                            to: vt,
                            length: int_at(f, "value_length", -1),
                            precision: int_at(f, "value_precision", -1),
                        }
                    } else {
                        l.expr
                    };
                    p.set(
                        &target,
                        e,
                        Field {
                            name: target.clone(),
                            ty,
                            length: int_at(f, "value_length", -1),
                            precision: int_at(f, "value_precision", -1),
                        },
                    );
                }
                node(Op::Project { items: p.items }, p.schema, acc)
            }
            "FilterRows" => {
                let pred = match x.at("compare/condition") {
                    Some(c) => condition::lower(c, &type_of(schema))?,
                    None => Lowered::exact(Expr::bool(true)),
                };
                acc.absorb(&pred);
                routed(x, input, pred.expr, acc)
            }
            "JavaFilter" => {
                let src = x
                    .raw_at("condition")
                    .ok_or_else(|| Gap::new("Java filter without a condition"))?;
                let ft = |n: &str| schema.iter().find(|f| f.name == n).map(|f| f.ty);
                let (l, _) = java::lower_over_fields(src, &ft)?;
                acc.absorb(&l);
                routed(x, input, Expr::two_valued(l.expr), acc)
            }
            "SwitchCase" => switch_case(x, input, schema, acc),
            "Constant" | "SetValueConstant" | "ValueMapper" | "IfNull" | "NullIf"
            | "ReplaceString" | "StringOperations" | "NumberRange" => {
                let mut p = Projection::over(schema);
                match step.type_id.as_str() {
                    "Constant" => constants(x, &mut p)?,
                    "SetValueConstant" => set_value_constant(x, &mut p, scope, &mut acc)?,
                    "ValueMapper" => value_mapper(x, &mut p)?,
                    "IfNull" => if_null(x, &mut p, &mut acc)?,
                    "NullIf" => null_if(x, &mut p)?,
                    "ReplaceString" => replace_string(x, &mut p)?,
                    "StringOperations" => string_operations(x, &mut p)?,
                    _ => number_range(x, &mut p)?,
                }
                node(Op::Project { items: p.items }, p.schema, acc)
            }
            "Sequence" => {
                if x.flag_at("use_database", false) {
                    return Err(Gap::new("Add sequence from a database sequence"));
                }
                let name = x.text_at("valuename").unwrap_or("valuename").to_string();
                let start = x
                    .text_at("start_at")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                let increment = x
                    .text_at("increment_by")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                let order = self.upstream_sort(input);
                if order.is_empty() {
                    acc.note("Add sequence numbers rows in arrival order; with no Sort rows upstream the SQL numbers them in an arbitrary order");
                }
                if x.text_at("max_value")
                    .and_then(|v| v.parse::<i64>().ok())
                    .is_some_and(|m| m < 999_999_999)
                {
                    acc.note("the sequence's maximum (Kettle wraps around) is not applied");
                }
                let mut out = schema.to_vec();
                out.push(Field::new(&name, KType::Integer));
                node(
                    Op::Window {
                        partition: vec![],
                        order,
                        items: vec![(name, WindowFn::Sequence { start, increment })],
                    },
                    out,
                    acc,
                )
            }
            "SortRows" => {
                let keys = sort_keys(x, schema)?;
                if x.flag_at("unique_rows", false) {
                    acc.note_only("Sort rows also removes rows with equal sort keys");
                    let distinct_keys = keys.iter().map(|k| k.field.clone()).collect();
                    // A distinct on the keys, then the sort; one node each would lose nothing,
                    // but the distinct alone is what changes the rows.
                    return node(
                        Op::Distinct {
                            keys: distinct_keys,
                            count: None,
                        },
                        schema.to_vec(),
                        acc,
                    );
                }
                if keys.iter().any(|k| !k.case_sensitive) {
                    acc.note("case-insensitive sort keys: ordering compares lower-cased values");
                }
                node(Op::Sort { keys }, schema.to_vec(), acc)
            }
            "Unique" | "UniqueRowsByHashSet" => {
                if x.flag_at("reject_duplicate_row", false) {
                    return Err(Gap::new("duplicates are sent to an error hop"));
                }
                let keys: Vec<String> = x
                    .at("fields")
                    .map(|f| {
                        f.children_named("field")
                            .filter_map(|k| k.text_at("name").map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                for k in &keys {
                    need(schema, k, "key")?;
                }
                let count = (step.type_id == "Unique" && x.flag_at("count_rows", false))
                    .then(|| x.text_at("count_field").unwrap_or("count").to_string());
                if step.type_id == "Unique" {
                    let sorted = self.upstream_sort(input);
                    let covered = !keys.is_empty()
                        && keys.iter().all(|k| sorted.iter().any(|s| &s.field == k));
                    if !covered {
                        acc.note("Unique rows removes only consecutive duplicates; the input is not sorted on the keys here, so the SQL (one row per key) can return fewer rows than Kettle");
                    }
                }
                let mut out = schema.to_vec();
                if let Some(c) = &count {
                    out.push(Field::new(c, KType::Integer));
                }
                node(Op::Distinct { keys, count }, out, acc)
            }
            "GroupBy" | "MemoryGroupBy" => {
                if x.flag_at("all_rows", false) {
                    return Err(Gap::new(
                        "Group by with \"include all rows\" (aggregates attached to every row)",
                    ));
                }
                if x.flag_at("add_linenr", false) {
                    return Err(Gap::new("Group by with a line number per group"));
                }
                let group: Vec<String> = x
                    .at("group")
                    .map(|g| {
                        g.children_named("field")
                            .filter_map(|f| f.text_at("name").map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                for g in &group {
                    need(schema, g, "group field")?;
                }
                let mut aggs = Vec::new();
                for f in x
                    .at("fields")
                    .map(|f| f.children_named("field").collect::<Vec<_>>())
                    .unwrap_or_default()
                {
                    aggs.push(aggregate(f, schema, &mut acc)?);
                }
                if step.type_id == "GroupBy" && !group.is_empty() {
                    let sorted = self.upstream_sort(input);
                    let covered = group.iter().all(|g| sorted.iter().any(|s| &s.field == g));
                    if !covered {
                        acc.note("Group by groups consecutive rows; the input is not sorted on the group fields here, so Kettle would emit one row per run of equal keys, the SQL one per key");
                    }
                }
                let mut out: Vec<Field> = group
                    .iter()
                    .map(|g| {
                        schema
                            .iter()
                            .find(|f| &f.name == g)
                            .cloned()
                            .expect("checked")
                    })
                    .collect();
                out.extend(aggs.iter().map(|a| Field::new(&a.name, a.ty)));
                node(
                    Op::Aggregate {
                        group_by: group,
                        aggs,
                    },
                    out,
                    acc,
                )
            }
            "AnalyticQuery" => {
                let partition: Vec<String> = x
                    .at("group")
                    .map(|g| {
                        g.children_named("field")
                            .filter_map(|f| f.text_at("name").map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut items = Vec::new();
                let mut out = schema.to_vec();
                for f in x
                    .at("fields")
                    .map(|f| f.children_named("field").collect::<Vec<_>>())
                    .unwrap_or_default()
                {
                    let name = f.text_at("aggregate").unwrap_or_default().to_string();
                    let subject = f.text_at("subject").unwrap_or_default().to_string();
                    need(schema, &subject, "subject")?;
                    let offset = f
                        .text_at("valuefield")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1);
                    let w = match f.text_at("type").unwrap_or_default() {
                        "LAG" => WindowFn::Lag {
                            subject: subject.clone(),
                            offset,
                        },
                        "LEAD" => WindowFn::Lead {
                            subject: subject.clone(),
                            offset,
                        },
                        other => return Err(Gap::new(format!("analytic function {other}"))),
                    };
                    out.push(Field {
                        name: name.clone(),
                        ..schema
                            .iter()
                            .find(|s| s.name == subject)
                            .cloned()
                            .expect("checked")
                    });
                    items.push((name, w));
                }
                let order = self.upstream_sort(input);
                if order.is_empty() {
                    acc.note("Analytic query reads rows in arrival order; there is no Sort rows upstream, so LAG/LEAD order is arbitrary in SQL");
                }
                node(
                    Op::Window {
                        partition,
                        order,
                        items,
                    },
                    out,
                    acc,
                )
            }
            "SystemInfo" => {
                let mut p = Projection::over(schema);
                for f in x
                    .at("fields")
                    .map(|f| f.children_named("field").collect::<Vec<_>>())
                    .unwrap_or_default()
                {
                    let name = f.text_at("name").unwrap_or_default();
                    let (e, ty) = match f.text_at("type").unwrap_or_default() {
                        "system date (variable)" | "system date (fixed)" | "start date range" => {
                            (Expr::call(Func::Now, vec![]), KType::Date)
                        }
                        other => return Err(Gap::new(format!("system information `{other}`"))),
                    };
                    acc.note("the system date is the query's start time (Kettle takes it when the step starts)");
                    p.set(name, e, Field::new(name, ty));
                }
                node(Op::Project { items: p.items }, p.schema, acc)
            }
            _ => Err(Builder::category_gap(step)),
        }
    }

    /// Modified JavaScript Value, User Defined Java Class, or a compiled plugin's decompiled
    /// `processRow` (`plugin`): symbolic execution into a Project, plus a Filter when the code
    /// drops rows (RFC 0203).
    fn code_step(
        &mut self,
        step: &Step,
        prefix: &str,
        input: NodeId,
        plugin: Option<String>,
    ) -> Result<Lowering, Gap> {
        use ekos_lakehouse_kettle_expr::script::{Executor, Lang};
        use ekos_lakehouse_kettle_expr::stmt::{parse_class_body, parse_script};
        let x = &step.xml;
        let schema = self.ir.nodes[input].schema.clone();
        let ft = |n: &str| schema.iter().find(|f| f.name == n).map(|f| f.ty);
        let mut acc = Acc::new();
        // (output name, variable or field the value comes from, type, length, precision, replaces)
        let mut declared: Vec<(String, String, KType, i32, i32, bool)> = Vec::new();
        let outcome = if step.type_id == "ScriptValueMod" {
            if x.flag_at("compatible", false) {
                return Err(Gap::new(
                    "JavaScript in compatibility mode (the Kettle 2 API)",
                ));
            }
            let scripts: Vec<(i32, String)> = x
                .at("jsScripts")
                .map(|s| {
                    s.children_named("jsScript")
                        .map(|j| {
                            (
                                int_at(j, "jsScript_type", 0),
                                j.raw_at("jsScript_script").unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let meaningful = |src: &str| {
                src.lines()
                    .map(|l| l.split("//").next().unwrap_or("").trim())
                    .any(|l| !l.is_empty())
            };
            let transforms: Vec<&String> = scripts
                .iter()
                .filter(|(t, s)| *t == 0 && meaningful(s))
                .map(|(_, s)| s)
                .collect();
            if transforms.len() > 1 {
                return Err(Gap::new(
                    "several transform scripts (one calls functions of another)",
                ));
            }
            if scripts.iter().any(|(t, s)| *t == 1 && meaningful(s)) {
                return Err(Gap::new(
                    "a start script runs once and keeps values across rows",
                ));
            }
            if scripts.iter().any(|(t, s)| *t == 2 && meaningful(s)) {
                acc.note("the end script (run once after the last row) is not translated");
            }
            let src = transforms.first().map(|s| s.as_str()).unwrap_or("");
            let stmts = parse_script(src, true)?;
            for f in x
                .child("fields")
                .map(|f| f.children_named("field").collect::<Vec<_>>())
                .unwrap_or_default()
            {
                let var = f.text_at("name").unwrap_or_default().to_string();
                let out = f.text_at("rename").unwrap_or(&var).to_string();
                let ty = f.text_at("type").map(KType::parse).unwrap_or(KType::String);
                declared.push((
                    out,
                    var,
                    ty,
                    int_at(f, "length", -1),
                    int_at(f, "precision", -1),
                    f.flag_at("replace", false),
                ));
            }
            Executor::new(Lang::Js, &ft).run(&stmts)?
        } else {
            let src = match &plugin {
                Some(src) => src.clone(),
                None => {
                    if x.child("info_steps")
                        .is_some_and(|i| !i.children.is_empty())
                        || x.child("target_steps")
                            .is_some_and(|t| !t.children.is_empty())
                    {
                        return Err(Gap::new(
                            "the class reads info steps or writes target steps (several streams)",
                        ));
                    }
                    x.at("definitions")
                        .and_then(|d| {
                            d.children_named("definition")
                                .find(|d| d.text_at("class_type") == Some("TRANSFORM_CLASS"))
                        })
                        .and_then(|d| d.raw_at("class_source"))
                        .ok_or_else(|| Gap::new("no transform class"))?
                        .to_string()
                }
            };
            let body = parse_class_body(&src)?;
            let Some((_, stmts)) = body.methods.iter().find(|(n, _)| n == "processRow") else {
                return Err(Gap::new("the class has no processRow method"));
            };
            for f in x
                .child("fields")
                .map(|f| f.children_named("field").collect::<Vec<_>>())
                .unwrap_or_default()
            {
                let name = f.text_at("field_name").unwrap_or_default().to_string();
                let ty = f
                    .text_at("field_type")
                    .map(KType::parse)
                    .unwrap_or(KType::String);
                declared.push((
                    name.clone(),
                    name,
                    ty,
                    int_at(f, "field_length", -1),
                    int_at(f, "field_precision", -1),
                    false,
                ));
            }
            let mut ex = Executor::new(Lang::Java, &ft);
            if let Some(ps) = x.child("parameters") {
                for p in ps.children_named("parameter") {
                    if let Some(tag) = p.text_at("parameter_tag") {
                        ex.params.insert(
                            tag.to_string(),
                            p.at("parameter_value")
                                .map(|v| v.text.clone())
                                .unwrap_or_default(),
                        );
                    }
                }
            }
            let o = ex.run(stmts)?;
            // Fields the code sets that the step does not declare: existing fields it overwrites.
            for (name, (_, t)) in &o.outputs {
                if !declared.iter().any(|d| &d.0 == name) {
                    if schema.iter().any(|f| &f.name == name) {
                        declared.push((name.clone(), name.clone(), t.ktype(), -1, -1, true));
                    } else if plugin.is_some() {
                        // A compiled plugin declares its fields in its Meta class's code, not in
                        // the step XML: the field the code sets is the output.
                        acc.note(format!(
                            "`{name}` is a field the plugin's code adds (typed from the code)"
                        ));
                        declared.push((name.clone(), name.clone(), t.ktype(), -1, -1, false));
                    } else {
                        return Err(Gap::new(format!(
                            "the code sets `{name}`, which the step does not declare"
                        )));
                    }
                }
            }
            o
        };
        acc.absorb(&outcome.lowered);
        if plugin.is_some() {
            acc.note("translated from decompiled code: a reconstruction of the compiled plugin, not its source");
        }
        let mut p = Projection::over(&schema);
        for (out, var, ty, len, prec, replace) in &declared {
            let value = outcome
                .outputs
                .get(var)
                .or_else(|| outcome.vars.get(var))
                .map(|(e, _)| e.clone());
            let value = match value {
                Some(v) => v,
                None if schema.iter().any(|f| &f.name == var) => Expr::col(var),
                None => {
                    acc.note(format!("`{var}` is never assigned: it is null"));
                    Expr::null()
                }
            };
            let inferred = outcome
                .outputs
                .get(var)
                .or_else(|| outcome.vars.get(var))
                .map(|(_, t)| t.ktype())
                .unwrap_or(KType::Unknown);
            let e = if *ty != KType::Unknown && inferred != *ty {
                Expr::Cast {
                    arg: Box::new(value),
                    to: *ty,
                    length: *len,
                    precision: *prec,
                }
            } else {
                value
            };
            if *replace && !schema.iter().any(|f| &f.name == out) {
                return Err(Gap::new(format!(
                    "`{out}` replaces a field that is not in the input"
                )));
            }
            if !*replace
                && schema.iter().any(|f| &f.name == out)
                && step.type_id == "ScriptValueMod"
            {
                return Err(Gap::new(format!(
                    "`{out}` is added but the input already has it (set Replace)"
                )));
            }
            p.set(
                out,
                e,
                Field {
                    name: out.clone(),
                    ty: *ty,
                    length: *len,
                    precision: *prec,
                },
            );
        }
        let Some(drop) = outcome.drop_when else {
            return Ok(Lowering::Node(
                Op::Project { items: p.items },
                vec![input],
                p.schema,
                acc.fidelity,
                acc.notes,
            ));
        };
        // The values first, then the rows the code keeps. The filter reads input fields only, so
        // it can follow the projection (which keeps them).
        let project = self.push(
            step,
            prefix,
            None,
            Op::Project { items: p.items },
            vec![input],
            p.schema.clone(),
            acc.fidelity,
            acc.notes.clone(),
        );
        let keep = drop.map_columns(&|c| Expr::col(c));
        Ok(Lowering::Node(
            Op::Filter {
                predicate: Expr::negate(keep),
            },
            vec![project],
            p.schema,
            acc.fidelity,
            acc.notes,
        ))
    }

    /// The sort keys of the nearest Sort rows upstream through row-preserving nodes.
    pub(crate) fn upstream_sort(&self, mut id: NodeId) -> Vec<SortKey> {
        loop {
            let n = &self.ir.nodes[id];
            match &n.op {
                Op::Sort { keys } => return keys.clone(),
                Op::Project { .. } | Op::Filter { .. } | Op::Window { .. }
                    if n.inputs.len() == 1 =>
                {
                    id = n.inputs[0]
                }
                _ => return Vec::new(),
            }
        }
    }

    fn table_input(
        &mut self,
        t: &Transformation,
        step: &Step,
        scope: &Scope,
        inputs: &[Input],
    ) -> Result<Lowering, Gap> {
        let x = &step.xml;
        if x.flag_at("execute_each_row", false)
            || x.text_at("lookup").is_some()
            || !inputs.is_empty()
        {
            return Err(Gap::new(
                "Table input runs once per incoming row with its values as parameters",
            ));
        }
        let mut acc = Acc::new();
        let conn_name = x.text_at("connection").unwrap_or_default();
        let connection = self.package.connection(&t.connections, conn_name).cloned();
        if connection.is_none() {
            return Err(Gap::new(format!(
                "connection `{conn_name}` is not defined in the file or shared.xml"
            )));
        }
        let raw = x
            .raw_at("sql")
            .ok_or_else(|| Gap::new("Table input without SQL"))?;
        let sql = if x.flag_at("variables_active", false) {
            let r = scope.resolve(raw);
            if !r.is_complete() {
                return Err(Gap::new(format!(
                    "the SQL uses variables with no static value: {}",
                    r.unresolved.join(", ")
                )));
            }
            if !r.used.is_empty() {
                acc.note(format!(
                    "variables replaced by their static values: {}",
                    r.used
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            r.text
        } else {
            raw.to_string()
        };
        let parsed = source::parse(&sql, connection.as_ref(), self.catalog)?;
        if !parsed.untyped.is_empty() {
            acc.note(format!(
                "types not known for: {} (computed in the query)",
                parsed.untyped.join(", ")
            ));
        }
        let limit = x
            .text_at("limit")
            .and_then(|l| l.parse::<u64>().ok())
            .filter(|l| *l > 0);
        if limit.is_some() {
            acc.note("Table input's row limit keeps whichever rows come first");
        }
        Ok(Lowering::Node(
            Op::Source {
                query: SourceQuery::Sql {
                    text: sql,
                    tables: parsed.tables,
                    limit,
                },
            },
            vec![],
            parsed.fields,
            acc.fidelity,
            acc.notes,
        ))
    }

    fn append(&mut self, x: &Node, inputs: &[Input]) -> Result<Lowering, Gap> {
        let head = x.text_at("head_name").unwrap_or_default();
        let tail = x.text_at("tail_name").unwrap_or_default();
        let find = |n: &str| {
            inputs
                .iter()
                .find(|i| i.from == n)
                .map(|i| i.node)
                .ok_or_else(|| Gap::new(format!("Append input `{n}` is not connected")))
        };
        let (h, tl) = (find(head)?, find(tail)?);
        let schema = self.ir.nodes[h].schema.clone();
        Ok(Lowering::Node(
            Op::Union,
            vec![h, tl],
            schema,
            Fidelity::Exact,
            vec![],
        ))
    }

    fn merge_join(&mut self, x: &Node, inputs: &[Input]) -> Result<Lowering, Gap> {
        let find = |n: &str| {
            inputs
                .iter()
                .find(|i| i.from == n)
                .map(|i| i.node)
                .ok_or_else(|| Gap::new(format!("Merge join input `{n}` is not connected")))
        };
        let l = find(x.text_at("step1").unwrap_or_default())?;
        let r = find(x.text_at("step2").unwrap_or_default())?;
        let keys = |p: &str| {
            x.at(p)
                .map(|k| {
                    k.children_named("key")
                        .map(|k| k.text.trim().to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let (k1, k2) = (keys("keys_1"), keys("keys_2"));
        if k1.len() != k2.len() || k1.is_empty() {
            return Err(Gap::new(
                "Merge join key lists differ in length or are empty",
            ));
        }
        let (ls, rs) = (
            self.ir.nodes[l].schema.clone(),
            self.ir.nodes[r].schema.clone(),
        );
        for k in &k1 {
            need(&ls, k, "key")?;
        }
        for k in &k2 {
            need(&rs, k, "key")?;
        }
        let kind = match x.text_at("join_type").unwrap_or("INNER") {
            "INNER" => JoinKind::Inner,
            "LEFT OUTER" => JoinKind::Left,
            "RIGHT OUTER" => JoinKind::Right,
            "FULL OUTER" => JoinKind::Full,
            other => return Err(Gap::new(format!("join type {other}"))),
        };
        let mut acc = Acc::new();
        if self.upstream_sort(l).is_empty() || self.upstream_sort(r).is_empty() {
            acc.note("Merge join expects both inputs sorted on the keys; Kettle's result on unsorted input differs, the SQL join does not depend on order");
        }
        let schema = joined(&ls, &rs, &mut acc);
        Ok(Lowering::Node(
            Op::Join {
                kind,
                keys: k1.into_iter().zip(k2).collect(),
                condition: None,
            },
            vec![l, r],
            schema,
            acc.fidelity,
            acc.notes,
        ))
    }

    fn join_rows(&mut self, x: &Node, inputs: &[Input]) -> Result<Lowering, Gap> {
        let [a, b] = inputs else {
            return Err(Gap::new(format!(
                "Join rows over {} inputs (two are translated)",
                inputs.len()
            )));
        };
        let (ls, rs) = (
            self.ir.nodes[a.node].schema.clone(),
            self.ir.nodes[b.node].schema.clone(),
        );
        let mut acc = Acc::new();
        let schema = joined(&ls, &rs, &mut acc);
        let condition = match x.at("compare/condition") {
            Some(c) if c.text_at("leftvalue").is_some() || c.child("conditions").is_some() => {
                let l = condition::lower(c, &type_of(&schema))?;
                acc.absorb(&l);
                Some(l.expr)
            }
            _ => None,
        };
        Ok(Lowering::Node(
            Op::Join {
                kind: JoinKind::Cross,
                keys: vec![],
                condition,
            },
            vec![a.node, b.node],
            schema,
            acc.fidelity,
            acc.notes,
        ))
    }

    fn stream_lookup(&mut self, x: &Node, inputs: &[Input]) -> Result<Lowering, Gap> {
        let from = x.text_at("from").unwrap_or_default();
        let lookup = inputs
            .iter()
            .find(|i| i.from == from)
            .ok_or_else(|| Gap::new(format!("lookup step `{from}` is not connected")))?;
        let mains: Vec<&Input> = inputs.iter().filter(|i| i.from != from).collect();
        let [main] = mains.as_slice() else {
            return Err(Gap::new("Stream lookup needs exactly one main input"));
        };
        let (ms, ls) = (
            self.ir.nodes[main.node].schema.clone(),
            self.ir.nodes[lookup.node].schema.clone(),
        );
        let l = x
            .child("lookup")
            .ok_or_else(|| Gap::new("Stream lookup without keys"))?;
        let mut keys = Vec::new();
        for k in l.children_named("key") {
            let (s, f) = (
                k.text_at("name").unwrap_or_default(),
                k.text_at("field").unwrap_or_default(),
            );
            need(&ms, s, "key")?;
            need(&ls, f, "lookup key")?;
            keys.push((s.to_string(), f.to_string()));
        }
        let values = lookup_values(l, &ls)?;
        let mut acc = Acc::new();
        acc.note("Stream lookup keeps one row per key (the last one read when the lookup stream has duplicates); the SQL keeps one of them");
        let mut schema = ms.clone();
        schema.extend(values.iter().map(|v| Field::new(&v.rename, v.ty)));
        Ok(Lowering::Node(
            Op::Lookup {
                table: None,
                keys,
                values,
                valid_at: None,
                inner: false,
            },
            vec![main.node, lookup.node],
            schema,
            acc.fidelity,
            acc.notes,
        ))
    }

    fn table_ref(
        &self,
        t: &Transformation,
        x: &Node,
        schema_path: &str,
        table_path: &str,
    ) -> Result<TableRef, Gap> {
        let conn_name = x.text_at("connection").unwrap_or_default();
        let connection = self.package.connection(&t.connections, conn_name).cloned();
        if connection.is_none() {
            return Err(Gap::new(format!("connection `{conn_name}` is not defined")));
        }
        let table = x
            .text_at(table_path)
            .ok_or_else(|| Gap::new("no table named"))?
            .to_string();
        if table.contains("${") || table.contains("%%") {
            return Err(Gap::new(format!("the table name `{table}` is a variable")));
        }
        Ok(TableRef {
            schema: x
                .text_at(schema_path)
                .map(str::to_string)
                .or_else(|| connection.as_ref().and_then(|c| c.schema.clone())),
            connection,
            table,
        })
    }

    fn db_lookup(&mut self, t: &Transformation, x: &Node, input: NodeId) -> Result<Lowering, Gap> {
        let table = self.table_ref(t, x, "lookup/schema", "lookup/table")?;
        let ms = self.ir.nodes[input].schema.clone();
        let l = x.child("lookup").expect("table_ref read it");
        let cols = self.catalog.columns(&table);
        let mut keys = Vec::new();
        for k in l.children_named("key") {
            let cond = k.text_at("condition").unwrap_or("=");
            if cond != "=" {
                return Err(Gap::new(format!("Database lookup key condition `{cond}`")));
            }
            let s = k.text_at("name").unwrap_or_default();
            need(&ms, s, "key")?;
            keys.push((
                s.to_string(),
                k.text_at("field").unwrap_or_default().to_string(),
            ));
        }
        let mut values = Vec::new();
        for v in l.children_named("value") {
            let name = v.text_at("name").unwrap_or_default().to_string();
            let ty = v
                .text_at("type")
                .map(KType::parse)
                .filter(|t| *t != KType::Unknown)
                .or_else(|| {
                    cols.as_ref().and_then(|c| {
                        c.iter()
                            .find(|f| f.name.eq_ignore_ascii_case(&name))
                            .map(|f| f.ty)
                    })
                })
                .unwrap_or(KType::Unknown);
            let default = match v.text_at("default") {
                Some(d) => Some(literal(d, ty, None)?),
                None => None,
            };
            values.push(LookupValue {
                rename: v.text_at("rename").unwrap_or(&name).to_string(),
                name,
                default,
                ty,
            });
        }
        let mut acc = Acc::new();
        if l.flag_at("fail_on_multiple", false) {
            acc.note("Kettle fails the step when a key matches several rows; the SQL keeps one");
        } else {
            acc.note("several matching rows: Kettle takes the first by the step's order; the SQL keeps one");
        }
        let inner = l.flag_at("eat_row_on_failure", false);
        let mut schema = ms;
        schema.extend(values.iter().map(|v| Field::new(&v.rename, v.ty)));
        Ok(Lowering::Node(
            Op::Lookup {
                table: Some(table),
                keys,
                values,
                valid_at: None,
                inner,
            },
            vec![input],
            schema,
            acc.fidelity,
            acc.notes,
        ))
    }

    fn sink(&mut self, t: &Transformation, step: &Step, input: NodeId) -> Result<Lowering, Gap> {
        let x = &step.xml;
        let ms = self.ir.nodes[input].schema.clone();
        let mut acc = Acc::new();
        let (mode, target, columns, keys, truncate) = match step.type_id.as_str() {
            "TableOutput" => {
                let target = self.table_ref(t, x, "schema", "table")?;
                if x.flag_at("tablename_in_field", false) {
                    return Err(Gap::new("Table output takes the table name from a field"));
                }
                let columns = if x.flag_at("specify_fields", false) {
                    x.at("fields")
                        .map(|f| {
                            f.children_named("field")
                                .map(|c| {
                                    (
                                        c.text_at("stream_name").unwrap_or_default().to_string(),
                                        c.text_at("column_name").unwrap_or_default().to_string(),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    ms.iter()
                        .map(|f| (f.name.clone(), f.name.clone()))
                        .collect::<Vec<_>>()
                };
                (
                    SinkMode::Insert,
                    target,
                    columns,
                    vec![],
                    x.flag_at("truncate", false),
                )
            }
            "InsertUpdate" | "Update" | "Delete" => {
                let target = self.table_ref(t, x, "lookup/schema", "lookup/table")?;
                let l = x.child("lookup").expect("read");
                let mut keys = Vec::new();
                for k in l.children_named("key") {
                    let cond = k.text_at("condition").unwrap_or("=");
                    if cond != "=" {
                        return Err(Gap::new(format!("key condition `{cond}`")));
                    }
                    keys.push((
                        k.text_at("name").unwrap_or_default().to_string(),
                        k.text_at("field").unwrap_or_default().to_string(),
                    ));
                }
                let mut columns = Vec::new();
                for v in l.children_named("value") {
                    let col = v.text_at("name").unwrap_or_default().to_string();
                    let stream = v.text_at("rename").unwrap_or(&col).to_string();
                    if step.type_id == "InsertUpdate" && !v.flag_at("update", true) {
                        acc.note_only(&format!("`{col}` is written on insert only, never updated"));
                    }
                    columns.push((stream, col));
                }
                if x.flag_at("update_bypassed", false) {
                    acc.note_only("updates are skipped: existing rows are left as they are");
                }
                let mode = match step.type_id.as_str() {
                    "InsertUpdate" => SinkMode::Upsert,
                    "Update" => SinkMode::Update,
                    _ => SinkMode::Delete,
                };
                (mode, target, columns, keys, false)
            }
            _ => unreachable!("dispatched above"),
        };
        for (s, _) in columns.iter().chain(&keys) {
            need(&ms, s, "field")?;
        }
        if mode == SinkMode::Insert && truncate {
            acc.note_only("the table is truncated before each load: the model is rebuilt in full");
        }
        let sink = Sink {
            mode,
            target,
            columns,
            keys,
            truncate,
            scd: None,
        };
        Ok(Lowering::Node(
            Op::Sink { sink },
            vec![input],
            ms,
            acc.fidelity,
            acc.notes,
        ))
    }

    fn dimension(&mut self, t: &Transformation, x: &Node, input: NodeId) -> Result<Lowering, Gap> {
        let target = self.table_ref(t, x, "schema", "table")?;
        let ms = self.ir.nodes[input].schema.clone();
        let f = x
            .child("fields")
            .ok_or_else(|| Gap::new("Dimension lookup without fields"))?;
        let keys: Vec<(String, String)> = f
            .children_named("key")
            .map(|k| {
                (
                    k.text_at("name").unwrap_or_default().to_string(),
                    k.text_at("lookup").unwrap_or_default().to_string(),
                )
            })
            .collect();
        for (s, _) in &keys {
            need(&ms, s, "key")?;
        }
        let ret = f.child("return");
        let tk = ret
            .and_then(|r| r.text_at("name"))
            .ok_or_else(|| Gap::new("no technical key"))?
            .to_string();
        let tk_out = ret
            .and_then(|r| r.text_at("rename"))
            .unwrap_or(&tk)
            .to_string();
        let date = f.child("date");
        let from = date
            .and_then(|d| d.text_at("from"))
            .unwrap_or("date_from")
            .to_string();
        let to = date
            .and_then(|d| d.text_at("to"))
            .unwrap_or("date_to")
            .to_string();
        let stream_date = date.and_then(|d| d.text_at("name")).map(str::to_string);
        let mut acc = Acc::new();
        if !x.flag_at("update", false) {
            // A lookup of the technical key, valid at the stream date.
            let values = vec![LookupValue {
                name: tk.clone(),
                rename: tk_out.clone(),
                default: Some(Expr::int(0)),
                ty: KType::Integer,
            }];
            if stream_date.is_none() {
                acc.note("no stream date: the version valid now is used");
            }
            let mut schema = ms;
            schema.push(Field::new(&tk_out, KType::Integer));
            return Ok(Lowering::Node(
                Op::Lookup {
                    table: Some(target),
                    keys,
                    values,
                    valid_at: Some((stream_date, from, to)),
                    inner: false,
                },
                vec![input],
                schema,
                acc.fidelity,
                acc.notes,
            ));
        }
        let mut columns = Vec::new();
        let mut field_modes = Vec::new();
        for fl in f.children_named("field") {
            let s = fl.text_at("name").unwrap_or_default().to_string();
            need(&ms, &s, "field")?;
            let col = fl.text_at("lookup").unwrap_or(&s).to_string();
            let mode = match fl.text_at("update").unwrap_or("Insert") {
                "Insert" => "insert",
                "Update" => "update",
                "Punch through" => "punch_through",
                other => return Err(Gap::new(format!("dimension field update type `{other}`"))),
            };
            field_modes.push((col.clone(), mode.to_string()));
            columns.push((s, col));
        }
        acc.note_only("a slowly changing dimension: written as a dbt snapshot (type 2 fields) or a merge (type 1 fields)");
        let scd = Scd {
            technical_key: tk,
            version: ret.and_then(|r| r.text_at("version")).map(str::to_string),
            date_from: Some(from),
            date_to: Some(to),
            field_modes,
        };
        let sink = Sink {
            mode: SinkMode::Dimension,
            target,
            columns,
            keys,
            truncate: false,
            scd: Some(scd),
        };
        let mut schema = ms;
        schema.push(Field::new(&tk_out, KType::Integer));
        Ok(Lowering::Node(
            Op::Sink { sink },
            vec![input],
            schema,
            acc.fidelity,
            acc.notes,
        ))
    }

    fn mapping(
        &mut self,
        t: &Transformation,
        step: &Step,
        prefix: &str,
        inputs: &[Input],
    ) -> Result<Lowering, Gap> {
        let link = self
            .package
            .links
            .iter()
            .find(|l| l.from == t.path && l.via == step.name);
        let Some(to) = link.and_then(|l| l.to.clone()) else {
            return Err(Gap::new("the sub-transformation is not in the package"));
        };
        if !self.depth_ok() {
            return Err(Gap::new("Mappings nested too deeply"));
        }
        let child = self
            .package
            .transformation(&to)
            .ok_or_else(|| Gap::new("sub-transformation not read"))?
            .clone();
        let input = self.single_input(step, prefix, inputs)?;
        let x = &step.xml;
        let in_map = x.at("mappings/input/mapping");
        let out_map = x.at("mappings/output/mapping");
        let pairs = |m: Option<&Node>| -> Vec<(String, String)> {
            m.map(|m| {
                m.children_named("connector")
                    .map(|c| {
                        (
                            c.text_at("parent").unwrap_or_default().to_string(),
                            c.text_at("child").unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
        };
        let (ins, outs) = (pairs(in_map), pairs(out_map));
        let schema = self.ir.nodes[input].schema.clone();
        // Rename parent → child going in.
        let rename_in: Vec<(String, Expr)> = schema
            .iter()
            .map(|f| {
                (
                    ins.iter()
                        .find(|(p, _)| p == &f.name)
                        .map(|(_, c)| c.clone())
                        .unwrap_or(f.name.clone()),
                    Expr::col(&f.name),
                )
            })
            .collect();
        let in_schema: Vec<Field> = rename_in
            .iter()
            .zip(&schema)
            .map(|((n, _), f)| Field {
                name: n.clone(),
                ..f.clone()
            })
            .collect();
        let child_prefix = format!("{prefix}{}/", step.name);
        let entry = self.push(
            step,
            &child_prefix,
            None,
            Op::Project { items: rename_in },
            vec![input],
            in_schema,
            Fidelity::Exact,
            vec!["into the sub-transformation".into()],
        );
        let mi = in_map
            .and_then(|m| m.text_at("output_step"))
            .map(str::to_string)
            .or_else(|| {
                child
                    .steps
                    .iter()
                    .find(|s| s.type_id == "MappingInput")
                    .map(|s| s.name.clone())
            })
            .ok_or_else(|| Gap::new("the sub-transformation has no Mapping input step"))?;
        self.depth += 1;
        let child_outs = self.walk(&child, &child_prefix, &BTreeMap::from([(mi, entry)]));
        self.depth -= 1;
        let mo = out_map
            .and_then(|m| m.text_at("input_step"))
            .map(str::to_string)
            .or_else(|| {
                child
                    .steps
                    .iter()
                    .find(|s| s.type_id == "MappingOutput")
                    .map(|s| s.name.clone())
            })
            .ok_or_else(|| Gap::new("the sub-transformation has no Mapping output step"))?;
        let exit = child_outs
            .get(&mo)
            .and_then(|o| o.main)
            .ok_or_else(|| Gap::new("the Mapping output step produced nothing"))?;
        if self.ir.nodes[exit].is_gap() {
            return Err(Gap::new(format!(
                "the sub-transformation `{to}` is not fully translated"
            )));
        }
        let rename_back = in_map.is_some_and(|m| m.flag_at("rename_on_output", false));
        let exit_schema = self.ir.nodes[exit].schema.clone();
        let back = |name: &str| -> String {
            if let Some((_, p)) = outs.iter().find(|(c, _)| c == name).map(|(c, p)| (c, p)) {
                // output connectors are (parent = child field, child = parent field) in Kettle's XML
                return p.clone();
            }
            if rename_back && let Some((p, _)) = ins.iter().find(|(_, c)| c == name) {
                return p.clone();
            }
            name.to_string()
        };
        let items: Vec<(String, Expr)> = exit_schema
            .iter()
            .map(|f| (back(&f.name), Expr::col(&f.name)))
            .collect();
        let out_schema: Vec<Field> = items
            .iter()
            .zip(&exit_schema)
            .map(|((n, _), f)| Field {
                name: n.clone(),
                ..f.clone()
            })
            .collect();
        let id = self.push(
            step,
            prefix,
            None,
            Op::Project { items },
            vec![exit],
            out_schema,
            Fidelity::Exact,
            vec![format!("sub-transformation `{to}` inlined")],
        );
        Ok(Lowering::Inlined(id))
    }
}

/// Left fields then right fields; a right field whose name is taken gets Kettle's `_1` suffix.
fn joined(l: &[Field], r: &[Field], acc: &mut Acc) -> Vec<Field> {
    let mut out = l.to_vec();
    for f in r {
        let mut f = f.clone();
        if out.iter().any(|x| x.name == f.name) {
            let renamed = format!("{}_1", f.name);
            acc.note_only(&format!(
                "`{}` appears on both sides; the right one is `{renamed}`, as Kettle names it",
                f.name
            ));
            f.name = renamed;
        }
        out.push(f);
    }
    out
}

fn routed(x: &Node, input: NodeId, pred: Expr, acc: Acc) -> Result<Lowering, Gap> {
    let t = x.text_at("send_true_to").map(str::to_string);
    let f = x.text_at("send_false_to").map(str::to_string);
    let branches = match (t, f) {
        (None, None) => vec![(None, pred)],
        (t, f) => {
            let mut b = Vec::new();
            if let Some(t) = t {
                b.push((Some(t), pred.clone()));
            }
            if let Some(f) = f {
                b.push((Some(f), Expr::negate(pred)));
            }
            b
        }
    };
    Ok(Lowering::Routed(branches, input, acc.fidelity, acc.notes))
}

fn switch_case(x: &Node, input: NodeId, schema: &[Field], mut acc: Acc) -> Result<Lowering, Gap> {
    let field = x
        .text_at("fieldname")
        .ok_or_else(|| Gap::new("Switch/case without a field"))?;
    need(schema, field, "field")?;
    let ft = type_of(schema)(field);
    let vt = x
        .text_at("case_value_type")
        .map(KType::parse)
        .filter(|t| *t != KType::Unknown)
        .unwrap_or(ft);
    let contains = x.flag_at("use_contains", false);
    let mut per_target: Vec<(String, Expr)> = Vec::new();
    for c in x
        .at("cases")
        .map(|c| c.children_named("case").collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let target = c.text_at("target_step").unwrap_or_default().to_string();
        let test = match c
            .at("value")
            .map(|v| v.text.as_str())
            .filter(|v| !v.is_empty())
        {
            None => Expr::is_null(Expr::col(field)),
            Some(v) if contains => Expr::two_valued(Expr::bin(
                BinOp::Gt,
                Expr::call(Func::Strpos, vec![Expr::col(field), Expr::str(v)]),
                Expr::int(0),
            )),
            Some(v) => {
                let lit = literal(v, vt, x.text_at("case_value_format"))?;
                Expr::two_valued(kettle_compare(
                    BinOp::Eq,
                    Expr::col(field),
                    lit,
                    true,
                    false,
                ))
            }
        };
        match per_target.iter_mut().find(|(t, _)| *t == target) {
            Some((_, e)) => *e = Expr::or(e.clone(), test),
            None => per_target.push((target, test)),
        }
    }
    let any = per_target.iter().map(|(_, e)| e.clone()).reduce(Expr::or);
    let mut branches: Vec<(Option<String>, Expr)> =
        per_target.into_iter().map(|(t, e)| (Some(t), e)).collect();
    if let Some(d) = x.text_at("default_target_step") {
        branches.push((
            Some(d.to_string()),
            any.map(Expr::negate).unwrap_or(Expr::bool(true)),
        ));
    }
    acc.note_only("each case becomes a filter on the same input");
    Ok(Lowering::Routed(branches, input, acc.fidelity, acc.notes))
}

fn select_values(x: &Node, schema: &[Field], acc: &mut Acc) -> Result<(Items, Vec<Field>), Gap> {
    let fields = x.child("fields");
    let selected: Vec<&Node> = fields
        .map(|f| f.children_named("field").collect())
        .unwrap_or_default();
    let mut items: Items;
    let mut out: Vec<Field>;
    if selected.is_empty() {
        items = passthrough(schema);
        out = schema.to_vec();
    } else {
        items = Vec::new();
        out = Vec::new();
        for s in &selected {
            let name = s.text_at("name").unwrap_or_default();
            need(schema, name, "field")?;
            let rename = s.text_at("rename").unwrap_or(name);
            let mut f = schema
                .iter()
                .find(|f| f.name == name)
                .cloned()
                .expect("checked");
            f.name = rename.to_string();
            let (len, prec) = (int_at(s, "length", -2), int_at(s, "precision", -2));
            if len >= 0 {
                f.length = len;
            }
            if prec >= 0 {
                f.precision = prec;
            }
            items.push((rename.to_string(), Expr::col(name)));
            out.push(f);
        }
        if fields.is_some_and(|f| f.flag_at("select_unspecified", false)) {
            for f in schema {
                if !selected
                    .iter()
                    .any(|s| s.text_at("name") == Some(f.name.as_str()))
                {
                    items.push((f.name.clone(), Expr::col(&f.name)));
                    out.push(f.clone());
                }
            }
        }
    }
    for r in fields
        .map(|f| f.children_named("remove").collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let name = r.text_at("name").unwrap_or_default();
        let Some(i) = items.iter().position(|(n, _)| n == name) else {
            return Err(Gap::new(format!(
                "removed field `{name}` is not in the step's input"
            )));
        };
        items.remove(i);
        out.remove(i);
    }
    for m in fields
        .map(|f| f.children_named("meta").collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let name = m.text_at("name").unwrap_or_default();
        let Some(i) = items.iter().position(|(n, _)| n == name) else {
            return Err(Gap::new(format!(
                "field `{name}` in the meta-data tab is not in the step's input"
            )));
        };
        let to = m
            .text_at("type")
            .map(KType::parse)
            .unwrap_or(KType::Unknown);
        let from = out[i].ty;
        let l = convert(items[i].1.clone(), from, to, m.text_at("conversion_mask"))?;
        acc.absorb(&l);
        items[i].1 = l.expr;
        if to != KType::Unknown {
            out[i].ty = to;
        }
        if let Some(r) = m.text_at("rename") {
            items[i].0 = r.to_string();
            out[i].name = r.to_string();
        }
        let (len, prec) = (int_at(m, "length", -2), int_at(m, "precision", -2));
        if len >= 0 {
            out[i].length = len;
        }
        if prec >= 0 {
            out[i].precision = prec;
        }
    }
    Ok((items, out))
}

fn field_list<'a>(x: &'a Node, container: &'a str) -> Vec<&'a Node> {
    x.child(container)
        .map(|f| f.children_named("field").collect())
        .unwrap_or_default()
}

fn constant_field(f: &Node) -> Result<(String, Expr, Field), Gap> {
    let name = f.text_at("name").unwrap_or_default().to_string();
    let ty = f.text_at("type").map(KType::parse).unwrap_or(KType::String);
    let ty = if ty == KType::Unknown {
        KType::String
    } else {
        ty
    };
    let raw = f.at("nullif").map(|n| n.text.as_str()).unwrap_or("");
    let e = if f.flag_at("set_empty_string", false) || raw.is_empty() {
        Expr::null()
    } else {
        literal(raw, ty, f.text_at("format"))?
    };
    let e = if matches!(e, Expr::Lit { .. }) && ty != KType::String {
        Expr::cast(e, ty)
    } else {
        e
    };
    Ok((
        name.clone(),
        e,
        Field {
            name,
            ty,
            length: int_at(f, "length", -1),
            precision: int_at(f, "precision", -1),
        },
    ))
}

fn constants(x: &Node, p: &mut Projection) -> Result<(), Gap> {
    for f in field_list(x, "fields") {
        let (n, e, fl) = constant_field(f)?;
        p.set(&n, e, fl);
    }
    Ok(())
}

fn set_value_constant(
    x: &Node,
    p: &mut Projection,
    scope: &Scope,
    acc: &mut Acc,
) -> Result<(), Gap> {
    let use_vars = x.flag_at("usevar", false);
    for f in field_list(x, "fields") {
        let name = f.text_at("name").unwrap_or_default();
        need(&p.schema, name, "field")?;
        let ty = p.ty()(name);
        let mut raw = f.at("value").map(|v| v.text.clone()).unwrap_or_default();
        if use_vars {
            let r = scope.resolve(&raw);
            if !r.is_complete() {
                return Err(Gap::new(format!(
                    "value uses variables with no static value: {}",
                    r.unresolved.join(", ")
                )));
            }
            if !r.used.is_empty() {
                acc.note("variables replaced by their static values");
            }
            raw = r.text;
        }
        let e = if f.flag_at("set_empty_string", false) || raw.is_empty() {
            Expr::null()
        } else {
            literal(&raw, ty, f.text_at("mask"))?
        };
        let fl = p
            .schema
            .iter()
            .find(|x| x.name == name)
            .cloned()
            .expect("checked");
        p.set(name, e, fl);
    }
    Ok(())
}

fn value_mapper(x: &Node, p: &mut Projection) -> Result<(), Gap> {
    let src = x
        .text_at("field_to_use")
        .ok_or_else(|| Gap::new("Value mapper without a field"))?;
    need(&p.schema, src, "field")?;
    let target = x.text_at("target_field").unwrap_or(src).to_string();
    let mut when = Vec::new();
    for f in field_list(x, "fields") {
        let from = f
            .at("source_value")
            .map(|v| v.text.clone())
            .unwrap_or_default();
        let to = f
            .at("target_value")
            .map(|v| v.text.clone())
            .unwrap_or_default();
        let test = if from.is_empty() {
            Expr::is_null(Expr::col(src))
        } else {
            Expr::bin(
                BinOp::Eq,
                Expr::cast(Expr::col(src), KType::String),
                Expr::str(&from),
            )
        };
        when.push((
            test,
            if to.is_empty() {
                Expr::null()
            } else {
                Expr::str(&to)
            },
        ));
    }
    let otherwise = match x.text_at("non_match_default") {
        Some(d) => Expr::str(d),
        // No default: an unmatched value is kept when the field is replaced, null otherwise.
        None if target == src => Expr::cast(Expr::col(src), KType::String),
        None => Expr::null(),
    };
    p.set(
        &target,
        Expr::case(when, Some(otherwise)),
        Field::new(&target, KType::String),
    );
    Ok(())
}

fn if_null(x: &Node, p: &mut Projection, acc: &mut Acc) -> Result<(), Gap> {
    let apply = |p: &mut Projection,
                 name: &str,
                 raw: &str,
                 mask: Option<&str>,
                 empty: bool|
     -> Result<(), Gap> {
        let ty = p.ty()(name);
        let v = if empty || raw.is_empty() {
            Expr::str("")
        } else {
            literal(raw, ty, mask)?
        };
        let fl = p
            .schema
            .iter()
            .find(|x| x.name == name)
            .cloned()
            .expect("checked");
        p.set(
            name,
            Expr::call(Func::Coalesce, vec![Expr::col(name), v]),
            fl,
        );
        Ok(())
    };
    if x.flag_at("selectFields", false) {
        for f in field_list(x, "fields") {
            let name = f.text_at("name").unwrap_or_default();
            need(&p.schema, name, "field")?;
            let raw = f.at("value").map(|v| v.text.clone()).unwrap_or_default();
            apply(
                p,
                name,
                &raw,
                f.text_at("mask"),
                f.flag_at("set_empty_string", false),
            )?;
        }
    } else if x.flag_at("selectValuesType", false) {
        let types: Vec<(KType, String, Option<String>)> = x
            .child("valuetypes")
            .map(|v| {
                v.children_named("valuetype")
                    .map(|t| {
                        (
                            KType::parse(t.text_at("name").unwrap_or_default()),
                            t.at("value").map(|v| v.text.clone()).unwrap_or_default(),
                            t.text_at("mask").map(str::to_string),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let names: Vec<(String, KType)> = p.schema.iter().map(|f| (f.name.clone(), f.ty)).collect();
        for (n, ty) in names {
            if let Some((_, raw, mask)) = types.iter().find(|(t, _, _)| *t == ty) {
                apply(p, &n, raw, mask.as_deref(), false)?;
            }
        }
    } else {
        let raw = x
            .at("replaceAllByValue")
            .map(|v| v.text.clone())
            .unwrap_or_default();
        let names: Vec<String> = p.schema.iter().map(|f| f.name.clone()).collect();
        for n in names {
            apply(
                p,
                &n,
                &raw,
                x.text_at("replaceAllMask"),
                x.flag_at("setEmptyStringAll", false),
            )?;
        }
    }
    acc.note_only("If field value is null: each null replaced by the value");
    Ok(())
}

fn null_if(x: &Node, p: &mut Projection) -> Result<(), Gap> {
    for f in field_list(x, "fields") {
        let name = f.text_at("name").unwrap_or_default();
        need(&p.schema, name, "field")?;
        let raw = f.at("value").map(|v| v.text.clone()).unwrap_or_default();
        let fl = p
            .schema
            .iter()
            .find(|x| x.name == name)
            .cloned()
            .expect("checked");
        let v = literal(&raw, fl.ty, None)?;
        p.set(name, Expr::call(Func::NullIf, vec![Expr::col(name), v]), fl);
    }
    Ok(())
}

fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn replace_string(x: &Node, p: &mut Projection) -> Result<(), Gap> {
    for f in field_list(x, "fields") {
        let name = f.text_at("in_stream_name").unwrap_or_default();
        need(&p.schema, name, "field")?;
        let out = f.text_at("out_stream_name").unwrap_or(name).to_string();
        if f.text_at("replace_field_by_string").is_some() {
            return Err(Gap::new("Replace in string by another field's value"));
        }
        let search = f
            .at("replace_string")
            .map(|v| v.text.clone())
            .unwrap_or_default();
        let by = if f.flag_at("set_empty_string", false) {
            String::new()
        } else {
            f.at("replace_by_string")
                .map(|v| v.text.clone())
                .unwrap_or_default()
        };
        let regex = matches!(f.text_at("use_regex"), Some("yes" | "Y"));
        let whole = matches!(f.text_at("whole_word"), Some("yes" | "Y"));
        let case = !matches!(f.text_at("case_sensitive"), Some("no" | "N"));
        let e = if !regex && !whole && case {
            Expr::call(
                Func::Replace,
                vec![Expr::col(name), Expr::str(&search), Expr::str(&by)],
            )
        } else {
            let mut pat = if regex {
                search.clone()
            } else {
                regex_escape(&search)
            };
            if whole {
                pat = format!("\\b{pat}\\b");
            }
            if !case {
                pat = format!("(?i){pat}");
            }
            // Java's replaceAll replacement uses $1; Trino's regexp_replace does too.
            Expr::call(
                Func::RegexpReplace,
                vec![Expr::col(name), Expr::str(&pat), Expr::str(&by)],
            )
        };
        p.set(&out, e, Field::new(&out, KType::String));
    }
    Ok(())
}

fn string_operations(x: &Node, p: &mut Projection) -> Result<(), Gap> {
    for f in field_list(x, "fields") {
        let name = f.text_at("in_stream_name").unwrap_or_default();
        need(&p.schema, name, "field")?;
        let out = f.text_at("out_stream_name").unwrap_or(name).to_string();
        let mut e = Expr::col(name);
        match f.text_at("trim_type").unwrap_or("none") {
            "left" => e = Expr::call(Func::LTrim, vec![e]),
            "right" => e = Expr::call(Func::RTrim, vec![e]),
            "both" => e = Expr::call(Func::Trim, vec![e]),
            _ => {}
        }
        match f.text_at("lower_upper").unwrap_or("none") {
            "lower" => e = Expr::call(Func::Lower, vec![e]),
            "upper" => e = Expr::call(Func::Upper, vec![e]),
            _ => {}
        }
        if f.flag_at("init_cap", false) || f.text_at("init_cap") == Some("yes") {
            e = Expr::call(Func::Initcap, vec![e]);
        }
        let pad_len: i64 = f
            .text_at("pad_len")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let pad = f
            .at("pad_char")
            .map(|v| v.text.clone())
            .unwrap_or_else(|| " ".into());
        match f.text_at("padding_type").unwrap_or("none") {
            "left" if pad_len > 0 => {
                e = Expr::call(Func::Lpad, vec![e, Expr::int(pad_len), Expr::str(&pad)])
            }
            "right" if pad_len > 0 => {
                e = Expr::call(Func::Rpad, vec![e, Expr::int(pad_len), Expr::str(&pad)])
            }
            _ => {}
        }
        match f.text_at("digits").unwrap_or("none") {
            "digits_only" | "only" => {
                e = Expr::call(
                    Func::RegexpReplace,
                    vec![e, Expr::str("[^0-9]"), Expr::str("")],
                )
            }
            "remove_digits" | "remove" => {
                e = Expr::call(
                    Func::RegexpReplace,
                    vec![e, Expr::str("[0-9]"), Expr::str("")],
                )
            }
            _ => {}
        }
        if !matches!(f.text_at("mask_xml").unwrap_or("none"), "none") {
            return Err(Gap::new("String operations: XML escaping"));
        }
        if !matches!(
            f.text_at("remove_special_characters").unwrap_or("none"),
            "none"
        ) {
            return Err(Gap::new("String operations: removing special characters"));
        }
        p.set(&out, e, Field::new(&out, KType::String));
    }
    Ok(())
}

fn number_range(x: &Node, p: &mut Projection) -> Result<(), Gap> {
    let input = x
        .text_at("inputField")
        .ok_or_else(|| Gap::new("Number range without an input field"))?;
    need(&p.schema, input, "field")?;
    let output = x.text_at("outputField").unwrap_or("range").to_string();
    let mut when = Vec::new();
    for r in x
        .child("rules")
        .map(|r| r.children_named("rule").collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let bound = |p: &str| {
            r.text_at(p)
                .map(|v| literal(v, KType::Number, None))
                .transpose()
        };
        let mut test: Option<Expr> = None;
        if let Some(lo) = bound("lower_bound")? {
            test = Some(Expr::bin(BinOp::GtEq, Expr::col(input), lo));
        }
        if let Some(hi) = bound("upper_bound")? {
            let t = Expr::bin(BinOp::Lt, Expr::col(input), hi);
            test = Some(match test {
                Some(l) => Expr::and(l, t),
                None => t,
            });
        }
        let value = r.at("value").map(|v| v.text.clone()).unwrap_or_default();
        when.push((test.unwrap_or(Expr::bool(true)), Expr::str(&value)));
    }
    let fallback = x
        .text_at("fallBackValue")
        .map(Expr::str)
        .unwrap_or(Expr::null());
    p.set(
        &output,
        Expr::case(when, Some(fallback)),
        Field::new(&output, KType::String),
    );
    Ok(())
}

fn sort_keys(x: &Node, schema: &[Field]) -> Result<Vec<SortKey>, Gap> {
    let mut keys = Vec::new();
    for f in field_list(x, "fields") {
        let name = f.text_at("name").unwrap_or_default();
        need(schema, name, "sort field")?;
        keys.push(SortKey {
            field: name.to_string(),
            ascending: f.flag_at("ascending", true),
            case_sensitive: f.flag_at("case_sensitive", true),
        });
    }
    Ok(keys)
}

fn aggregate(f: &Node, schema: &[Field], acc: &mut Acc) -> Result<Agg, Gap> {
    let name = f.text_at("aggregate").unwrap_or_default().to_string();
    let subject = f.text_at("subject").map(str::to_string);
    if let Some(s) = &subject {
        need(schema, s, "aggregated field")?;
    }
    let st = subject
        .as_deref()
        .map(type_of(schema))
        .unwrap_or(KType::Unknown);
    let arg = f.text_at("valuefield").map(str::to_string);
    let (func, ty) = match f.text_at("type").unwrap_or_default() {
        "SUM" => ("sum", st),
        "AVERAGE" => ("avg", KType::Number),
        "MIN" => ("min", st),
        "MAX" => ("max", st),
        "COUNT_ALL" => ("count", KType::Integer),
        "COUNT_ANY" => ("count_all", KType::Integer),
        "COUNT_DISTINCT" => ("count_distinct", KType::Integer),
        "CONCAT_COMMA" => {
            acc.note(
                "strings concatenated with \", \" in no particular order (Kettle: arrival order)",
            );
            ("string_agg", KType::String)
        }
        "CONCAT_STRING" => {
            acc.note("strings concatenated in no particular order (Kettle: arrival order)");
            ("string_agg", KType::String)
        }
        "FIRST" | "FIRST_INCL_NULL" | "LAST" | "LAST_INCL_NULL" => {
            acc.note("first/last value of a group depends on row order: the SQL takes any value of the group");
            (
                if f.text_at("type").unwrap_or_default().starts_with("FIRST") {
                    "first"
                } else {
                    "last"
                },
                st,
            )
        }
        "MEDIAN" | "PERCENTILE" => {
            acc.note(
                "median/percentile: Trino's approx_percentile is approximate, Kettle's is exact",
            );
            (
                if f.text_at("type") == Some("MEDIAN") {
                    "median"
                } else {
                    "percentile"
                },
                KType::Number,
            )
        }
        "STD_DEV" => {
            acc.note("standard deviation as the population formula (stddev_pop)");
            ("stddev", KType::Number)
        }
        other => return Err(Gap::new(format!("aggregate {other}"))),
    };
    let arg = if func == "string_agg" && arg.is_none() {
        Some(", ".to_string())
    } else {
        arg
    };
    Ok(Agg {
        name,
        func: func.to_string(),
        subject,
        arg,
        ty,
    })
}

fn lookup_values(l: &Node, ls: &[Field]) -> Result<Vec<LookupValue>, Gap> {
    let mut values = Vec::new();
    for v in l.children_named("value") {
        let name = v.text_at("name").unwrap_or_default().to_string();
        need(ls, &name, "lookup value")?;
        let ty = v
            .text_at("type")
            .map(KType::parse)
            .filter(|t| *t != KType::Unknown)
            .unwrap_or_else(|| type_of(ls)(&name));
        let default = match v.text_at("default") {
            Some(d) => Some(literal(d, ty, None)?),
            None => None,
        };
        values.push(LookupValue {
            rename: v.text_at("rename").unwrap_or(&name).to_string(),
            name,
            default,
            ty,
        });
    }
    Ok(values)
}

fn data_grid(x: &Node) -> Result<Lowering, Gap> {
    let fields = field_list(x, "fields");
    let schema: Vec<Field> = fields
        .iter()
        .map(|f| {
            let ty = f.text_at("type").map(KType::parse).unwrap_or(KType::String);
            Field {
                name: f.text_at("name").unwrap_or_default().to_string(),
                ty,
                length: int_at(f, "length", -1),
                precision: int_at(f, "precision", -1),
            }
        })
        .collect();
    let mut rows = Vec::new();
    for line in x
        .child("data")
        .map(|d| d.children_named("line").collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let items: Vec<&Node> = line.children_named("item").collect();
        let mut row = Vec::new();
        for (i, (f, def)) in schema.iter().zip(&fields).enumerate() {
            let text = items.get(i).map(|n| n.text.as_str()).unwrap_or("");
            let e = if text.is_empty() || def.flag_at("set_empty_string", false) {
                Expr::null()
            } else {
                literal(text, f.ty, def.text_at("format"))?
            };
            row.push(if f.ty != KType::String {
                Expr::cast(e, f.ty)
            } else {
                e
            });
        }
        rows.push(row);
    }
    Ok(Lowering::Node(
        Op::Values { rows },
        vec![],
        schema,
        Fidelity::Exact,
        vec![],
    ))
}

fn row_generator(x: &Node) -> Result<Lowering, Gap> {
    if x.flag_at("never_ending", false) {
        return Err(Gap::new("Generate rows without end (a streaming source)"));
    }
    let count: u64 = x
        .text_at("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let mut row = Vec::new();
    let mut schema = Vec::new();
    for f in field_list(x, "fields") {
        let (_, e, fl) = constant_field(f)?;
        row.push(e);
        schema.push(fl);
    }
    Ok(Lowering::Node(
        Op::Generate { row, count },
        vec![],
        schema,
        Fidelity::Exact,
        vec![],
    ))
}

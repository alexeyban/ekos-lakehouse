//! IR nodes as Trino SQL: one CTE per node, a model's inputs from outside it as `ref()`s.

use ekos_lakehouse_kettle_expr::{Expr, Gap, KType, Lit, quote_ident as q};
use ekos_lakehouse_kettle_rel::{Ir, JoinKind, NodeId, Op, RelNode, SortKey, WindowFn};
use std::collections::BTreeMap;

/// How a node outside the model, or a table, is read.
pub struct Inputs<'a> {
    /// A node of another model: the `ref()` that model is read by.
    pub node_ref: &'a dyn Fn(NodeId) -> Option<String>,
    /// A Source node's rewritten SQL (with refs).
    pub source_sql: &'a BTreeMap<NodeId, String>,
    /// A lookup or dimension table: its `ref()`.
    pub table_ref: &'a dyn Fn(&ekos_lakehouse_kettle_rel::TableRef) -> Result<String, Gap>,
}

/// The CTE name of a node.
pub fn cte_name(n: &RelNode) -> String {
    let slug: String = n
        .step
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let slug = slug.trim_matches('_').chars().take(40).collect::<String>();
    match &n.branch {
        Some(b) => {
            let b: String = b
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .take(20)
                .collect();
            format!("n{}_{slug}__{b}", n.id)
        }
        None => format!("n{}_{slug}", n.id),
    }
}

fn cols(fields: &[ekos_lakehouse_kettle_rel::Field]) -> String {
    fields
        .iter()
        .map(|f| q(&f.name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn order_by(keys: &[SortKey]) -> String {
    if keys.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = keys
        .iter()
        .map(|k| {
            let c = if k.case_sensitive {
                q(&k.field)
            } else {
                format!("lower({})", q(&k.field))
            };
            format!("{c} {}", if k.ascending { "ASC" } else { "DESC" })
        })
        .collect();
    format!("ORDER BY {}", parts.join(", "))
}

/// A value inside VALUES: a typed null so Trino can infer the column type.
fn value_sql(e: &Expr, ty: KType) -> String {
    match e {
        Expr::Lit { value: Lit::Null } => format!("CAST(NULL AS {})", ty.trino(-1, -1)),
        e => e.sql(),
    }
}

/// The SQL of one node, reading its inputs by `input(id)` (a CTE name or a ref).
pub fn node_sql(
    ir: &Ir,
    n: &RelNode,
    input: &dyn Fn(NodeId) -> String,
    inputs: &Inputs<'_>,
) -> Result<String, Gap> {
    let one = || {
        n.inputs
            .first()
            .map(|i| input(*i))
            .ok_or_else(|| Gap::new("no input"))
    };
    Ok(match &n.op {
        Op::Source { .. } => inputs
            .source_sql
            .get(&n.id)
            .cloned()
            .ok_or_else(|| Gap::new("the source query was not rewritten"))?,
        Op::Values { rows } => {
            if rows.is_empty() {
                let nulls: Vec<String> = n
                    .schema
                    .iter()
                    .map(|f| {
                        format!(
                            "CAST(NULL AS {}) AS {}",
                            f.ty.trino(f.length, f.precision),
                            q(&f.name)
                        )
                    })
                    .collect();
                format!("SELECT {} WHERE FALSE", nulls.join(", "))
            } else {
                let rows: Vec<String> = rows
                    .iter()
                    .map(|r| {
                        format!(
                            "({})",
                            r.iter()
                                .zip(&n.schema)
                                .map(|(e, f)| value_sql(e, f.ty))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
                    .collect();
                format!(
                    "SELECT * FROM (VALUES {}) AS t({})",
                    rows.join(", "),
                    cols(&n.schema)
                )
            }
        }
        Op::Generate { row, count } => {
            let items: Vec<String> = row
                .iter()
                .zip(&n.schema)
                .map(|(e, f)| format!("{} AS {}", value_sql(e, f.ty), q(&f.name)))
                .collect();
            format!(
                "SELECT {} FROM UNNEST(sequence(1, {count})) AS t(ekos_i)",
                items.join(", ")
            )
        }
        Op::Project { items } => {
            let items: Vec<String> = items
                .iter()
                .map(|(name, e)| format!("{} AS {}", e.sql(), q(name)))
                .collect();
            format!("SELECT {} FROM {}", items.join(", "), one()?)
        }
        Op::Filter { predicate } => format!("SELECT * FROM {} WHERE {}", one()?, predicate.sql()),
        Op::Sort { .. } => format!("SELECT * FROM {}", one()?),
        Op::Join {
            kind,
            keys,
            condition,
        } => {
            let (l, r) = (n.inputs[0], n.inputs[1]);
            let (ls, rs) = (&ir.node(l).schema, &ir.node(r).schema);
            let mut sel: Vec<String> = ls.iter().map(|f| format!("l.{}", q(&f.name))).collect();
            for (i, f) in rs.iter().enumerate() {
                let out = &n.schema[ls.len() + i].name;
                sel.push(format!("r.{} AS {}", q(&f.name), q(out)));
            }
            let join = match kind {
                JoinKind::Inner => "JOIN",
                JoinKind::Left => "LEFT JOIN",
                JoinKind::Right => "RIGHT JOIN",
                JoinKind::Full => "FULL JOIN",
                JoinKind::Cross => "CROSS JOIN",
            };
            let on = if keys.is_empty() {
                String::new()
            } else {
                let c: Vec<String> = keys
                    .iter()
                    .map(|(a, b)| format!("l.{} = r.{}", q(a), q(b)))
                    .collect();
                format!(" ON {}", c.join(" AND "))
            };
            let base = format!(
                "SELECT {} FROM {} AS l {join} {} AS r{on}",
                sel.join(", "),
                input(l),
                input(r)
            );
            match condition {
                Some(c) => format!("SELECT * FROM ({base}) AS j WHERE {}", c.sql()),
                None => base,
            }
        }
        Op::Lookup {
            table,
            keys,
            values,
            valid_at,
            inner,
        } => {
            let main = input(n.inputs[0]);
            let source = match (table, n.inputs.get(1)) {
                (Some(t), _) => (inputs.table_ref)(t)?,
                (None, Some(l)) => input(*l),
                (None, None) => return Err(Gap::new("lookup without a lookup input")),
            };
            let mut inner_cols: Vec<String> = keys.iter().map(|(_, k)| q(k)).collect();
            inner_cols.extend(values.iter().map(|v| q(&v.name)));
            if let Some((_, from, to)) = valid_at {
                inner_cols.push(q(from));
                inner_cols.push(q(to));
            }
            inner_cols.dedup();
            let mut on: Vec<String> = keys
                .iter()
                .map(|(s, k)| format!("m.{} = lk.{}", q(s), q(k)))
                .collect();
            let sub = match valid_at {
                Some((date, from, to)) => {
                    let d = date
                        .as_ref()
                        .map(|d| format!("m.{}", q(d)))
                        .unwrap_or_else(|| "current_timestamp".into());
                    on.push(format!("{d} >= lk.{} AND {d} < lk.{}", q(from), q(to)));
                    format!(
                        "SELECT {}, TRUE AS ekos_match FROM {source}",
                        inner_cols.join(", ")
                    )
                }
                None => {
                    on.push("lk.ekos_rn = 1".into());
                    let part: Vec<String> = keys.iter().map(|(_, k)| q(k)).collect();
                    format!(
                        "SELECT {}, TRUE AS ekos_match, row_number() OVER (PARTITION BY {}) AS ekos_rn FROM {source}",
                        inner_cols.join(", "),
                        part.join(", ")
                    )
                }
            };
            let mut sel = vec!["m.*".to_string()];
            for v in values {
                let d = v
                    .default
                    .as_ref()
                    .map(|d| d.sql())
                    .unwrap_or_else(|| "NULL".into());
                sel.push(format!(
                    "CASE WHEN lk.ekos_match THEN lk.{} ELSE {d} END AS {}",
                    q(&v.name),
                    q(&v.rename)
                ));
            }
            let join = if *inner { "JOIN" } else { "LEFT JOIN" };
            format!(
                "SELECT {} FROM {main} AS m {join} ({sub}) AS lk ON {}",
                sel.join(", "),
                on.join(" AND ")
            )
        }
        Op::Aggregate { group_by, aggs } => {
            let mut sel: Vec<String> = group_by.iter().map(|g| q(g)).collect();
            for a in aggs {
                let s = a.subject.as_deref().map(q).unwrap_or_else(|| "*".into());
                let e = match a.func.as_str() {
                    "count_all" => "count(*)".to_string(),
                    "count_distinct" => format!("count(DISTINCT {s})"),
                    "string_agg" => format!(
                        "array_join(array_agg(CAST({s} AS varchar)), {})",
                        ekos_lakehouse_kettle_expr::quote_str(a.arg.as_deref().unwrap_or(", "))
                    ),
                    "first" | "last" => format!("arbitrary({s})"),
                    "median" => format!("approx_percentile({s}, 0.5)"),
                    "percentile" => {
                        let p: f64 = a
                            .arg
                            .as_deref()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(50.0);
                        format!("approx_percentile({s}, {})", p / 100.0)
                    }
                    "stddev" => format!("stddev_pop({s})"),
                    f => format!("{f}({s})"),
                };
                sel.push(format!("{e} AS {}", q(&a.name)));
            }
            let gb = if group_by.is_empty() {
                String::new()
            } else {
                format!(
                    " GROUP BY {}",
                    group_by.iter().map(|g| q(g)).collect::<Vec<_>>().join(", ")
                )
            };
            format!("SELECT {} FROM {}{gb}", sel.join(", "), one()?)
        }
        Op::Window {
            partition,
            order,
            items,
        } => {
            let p = if partition.is_empty() {
                String::new()
            } else {
                format!(
                    "PARTITION BY {} ",
                    partition
                        .iter()
                        .map(|x| q(x))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let o = order_by(order);
            let over = format!("OVER ({p}{o})");
            let mut sel = vec!["*".to_string()];
            for (name, w) in items {
                let e = match w {
                    WindowFn::Sequence { start, increment } => {
                        format!("{start} + (row_number() {over} - 1) * {increment}")
                    }
                    WindowFn::Lag { subject, offset } => {
                        format!("lag({}, {offset}) {over}", q(subject))
                    }
                    WindowFn::Lead { subject, offset } => {
                        format!("lead({}, {offset}) {over}", q(subject))
                    }
                };
                sel.push(format!("{e} AS {}", q(name)));
            }
            format!("SELECT {} FROM {}", sel.join(", "), one()?)
        }
        Op::Distinct { keys, count } => {
            let input_schema = &ir.node(n.inputs[0]).schema;
            if keys.is_empty() && count.is_none() {
                format!("SELECT DISTINCT * FROM {}", one()?)
            } else {
                let part = if keys.is_empty() {
                    cols(input_schema)
                } else {
                    keys.iter().map(|k| q(k)).collect::<Vec<_>>().join(", ")
                };
                let cnt = count
                    .as_ref()
                    .map(|c| format!(", count(*) OVER (PARTITION BY {part}) AS {}", q(c)))
                    .unwrap_or_default();
                let outer = count
                    .as_ref()
                    .map(|c| format!("{}, {}", cols(input_schema), q(c)))
                    .unwrap_or_else(|| cols(input_schema));
                format!(
                    "SELECT {outer} FROM (SELECT *, row_number() OVER (PARTITION BY {part}) AS ekos_rn{cnt} FROM {}) AS d WHERE ekos_rn = 1",
                    one()?
                )
            }
        }
        Op::Union => {
            let first = &n.schema;
            let parts: Vec<String> = n
                .inputs
                .iter()
                .map(|i| format!("SELECT {} FROM {}", cols(first), input(*i)))
                .collect();
            parts.join(" UNION ALL ")
        }
        Op::Assert { .. } | Op::Sink { .. } => format!("SELECT * FROM {}", one()?),
        Op::Gap { reason } => return Err(Gap::new(reason.clone())),
    })
}

/// A model: the nodes `ids` (in IR order) as CTEs, then `final_select` over the last one.
pub fn model_sql(
    ir: &Ir,
    ids: &[NodeId],
    final_select: &str,
    inputs: &Inputs<'_>,
) -> Result<String, Gap> {
    let inside = |id: NodeId| ids.contains(&id);
    let mut ctes = Vec::new();
    for &id in ids {
        let n = ir.node(id);
        let input = |i: NodeId| -> String {
            if inside(i) {
                cte_name(ir.node(i))
            } else {
                (inputs.node_ref)(i).unwrap_or_else(|| format!("missing_input_{i}"))
            }
        };
        let sql =
            node_sql(ir, n, &input, inputs).map_err(|g| Gap::new(format!("`{}`: {g}", n.step)))?;
        if let Some(i) = n
            .inputs
            .iter()
            .find(|i| !inside(**i) && (inputs.node_ref)(**i).is_none())
        {
            return Err(Gap::new(format!(
                "`{}` reads node {i}, which no model provides",
                n.step
            )));
        }
        ctes.push(format!("{} AS (\n    {sql}\n)", cte_name(n)));
    }
    let last = ids
        .last()
        .map(|id| cte_name(ir.node(*id)))
        .ok_or_else(|| Gap::new("an empty model"))?;
    Ok(format!(
        "WITH\n{}\nSELECT {final_select} FROM {last}",
        ctes.join(",\n")
    ))
}

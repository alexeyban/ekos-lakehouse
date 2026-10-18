//! Walks a transformation's steps in hop order and lowers each into IR nodes (RFC 0202).
//!
//! The walk owns everything that is about the graph rather than one step: routed outputs,
//! error hops, round-robin distribution, implicit unions of several inputs, gaps propagating
//! downstream, and inlining of Mapping sub-transformations.

use crate::Catalog;
use crate::model::{Field, Ir, NodeId, Op, RelNode};
use ekos_lakehouse_kettle::links::file_scope;
use ekos_lakehouse_kettle::variables::Scope;
use ekos_lakehouse_kettle::{Category, Package, Step, Transformation};
use ekos_lakehouse_kettle_expr::{Expr, Fidelity, Gap};
use std::collections::{BTreeMap, BTreeSet};

/// Mapping inlining stops here: a deeper chain is reported, not followed.
const MAX_MAPPING_DEPTH: usize = 8;

/// What one step produced: its main output and, for a routing step, one node per target.
#[derive(Debug, Clone, Default)]
pub(crate) struct StepOut {
    pub main: Option<NodeId>,
    pub routed: BTreeMap<String, NodeId>,
    /// Rows go round-robin to the outputs: no output sees all of them.
    pub distributed: bool,
}

/// One input of a step: the step it comes from and the node that carries its rows.
#[derive(Debug, Clone)]
pub(crate) struct Input {
    pub from: String,
    pub node: NodeId,
}

/// What a step lowering returns. Short-lived (one per step), so its size does not matter.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Lowering {
    /// One node.
    Node(Op, Vec<NodeId>, Vec<Field>, Fidelity, Vec<String>),
    /// One node per target step (Filter rows, Switch/case, Java filter): `(target, predicate)`.
    /// `None` target: the rows every other hop gets.
    Routed(Vec<(Option<String>, Expr)>, NodeId, Fidelity, Vec<String>),
    /// A Mapping inlined: its output node.
    Inlined(NodeId),
}

pub(crate) struct Builder<'a> {
    pub package: &'a Package,
    pub catalog: &'a dyn Catalog,
    pub ir: Ir,
    pub depth: usize,
    /// Decompiled `processRow` source per plugin step type (RFC 0203).
    pub plugin_code: &'a BTreeMap<String, String>,
}

/// Builds the IR of `t`.
pub fn build(package: &Package, t: &Transformation, catalog: &dyn Catalog) -> Ir {
    build_with(package, t, catalog, &BTreeMap::new())
}

/// Builds the IR of `t`, with decompiled `processRow` source per plugin step type (RFC 0203).
pub fn build_with(
    package: &Package,
    t: &Transformation,
    catalog: &dyn Catalog,
    plugin_code: &BTreeMap<String, String>,
) -> Ir {
    let mut b = Builder {
        package,
        catalog,
        ir: Ir {
            transformation: t.path.clone(),
            nodes: Vec::new(),
        },
        depth: 0,
        plugin_code,
    };
    b.walk(t, "", &BTreeMap::new());
    b.ir
}

/// Steps in hop order; steps on a cycle come last, in file order, and are reported.
fn order(t: &Transformation) -> (Vec<&Step>, BTreeSet<String>) {
    let mut names: Vec<&str> = Vec::new();
    for s in &t.steps {
        if !names.contains(&s.name.as_str()) {
            names.push(&s.name);
        }
    }
    let mut indeg: BTreeMap<&str, usize> = names.iter().map(|n| (*n, 0)).collect();
    for h in t.hops.iter().filter(|h| h.enabled) {
        if indeg.contains_key(h.from.as_str())
            && let Some(d) = indeg.get_mut(h.to.as_str())
        {
            *d += 1;
        }
    }
    let mut ready: Vec<&str> = names.iter().copied().filter(|n| indeg[n] == 0).collect();
    let mut done = Vec::new();
    while let Some(n) = ready.first().copied() {
        ready.remove(0);
        if done.contains(&n) {
            continue;
        }
        done.push(n);
        for h in t.hops.iter().filter(|h| h.enabled && h.from == n) {
            if let Some(d) = indeg.get_mut(h.to.as_str())
                && *d > 0
            {
                *d -= 1;
                if *d == 0 {
                    ready.push(&h.to);
                }
            }
        }
    }
    let cyclic: BTreeSet<String> = names
        .iter()
        .filter(|n| !done.contains(n))
        .map(|n| n.to_string())
        .collect();
    let mut steps: Vec<&Step> = done.iter().filter_map(|n| t.step(n)).collect();
    steps.extend(t.steps.iter().filter(|s| cyclic.contains(&s.name)));
    (steps, cyclic)
}

impl Builder<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push(
        &mut self,
        step: &Step,
        prefix: &str,
        branch: Option<String>,
        op: Op,
        inputs: Vec<NodeId>,
        schema: Vec<Field>,
        fidelity: Fidelity,
        notes: Vec<String>,
    ) -> NodeId {
        let id = self.ir.nodes.len();
        let fidelity = if matches!(op, Op::Gap { .. }) {
            Fidelity::Gap
        } else {
            fidelity
        };
        self.ir.nodes.push(RelNode {
            id,
            step: format!("{prefix}{}", step.name),
            step_type: step.type_id.clone(),
            branch,
            op,
            inputs,
            schema,
            fidelity,
            notes,
            line: step.line,
        });
        id
    }

    pub(crate) fn gap(
        &mut self,
        step: &Step,
        prefix: &str,
        inputs: Vec<NodeId>,
        reason: impl Into<String>,
    ) -> NodeId {
        self.push(
            step,
            prefix,
            None,
            Op::Gap {
                reason: reason.into(),
            },
            inputs,
            Vec::new(),
            Fidelity::Gap,
            Vec::new(),
        )
    }

    /// Lowers every step of `t`. `mapping_inputs`: for an inlined Mapping, the node feeding
    /// each Mapping input step (by its name in `t`).
    pub(crate) fn walk(
        &mut self,
        t: &Transformation,
        prefix: &str,
        mapping_inputs: &BTreeMap<String, NodeId>,
    ) -> BTreeMap<String, StepOut> {
        let scope = file_scope(self.package, &t.path, &t.name, false, &t.parameters);
        let (steps, cyclic) = order(t);
        let mut outs: BTreeMap<String, StepOut> = BTreeMap::new();
        for step in steps {
            let out = self.step(t, step, prefix, &scope, &outs, &cyclic, mapping_inputs);
            outs.insert(step.name.clone(), out);
        }
        outs
    }

    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        t: &Transformation,
        step: &Step,
        prefix: &str,
        scope: &Scope,
        outs: &BTreeMap<String, StepOut>,
        cyclic: &BTreeSet<String>,
        mapping_inputs: &BTreeMap<String, NodeId>,
    ) -> StepOut {
        let data_outputs = t.outputs_of(&step.name);
        // Steps with typed target streams never distribute round-robin.
        let routing = matches!(
            step.type_id.as_str(),
            "FilterRows"
                | "SwitchCase"
                | "JavaFilter"
                | "TransExecutor"
                | "JobExecutor"
                | "SingleThreader"
                | "Validator"
        );
        let distributed = data_outputs.len() > 1 && step.distribute && !routing;
        let mut out = StepOut {
            distributed,
            ..StepOut::default()
        };
        if cyclic.contains(&step.name) {
            out.main = Some(self.gap(step, prefix, Vec::new(), "the step is on a cycle of hops"));
            return out;
        }
        // Resolve inputs.
        let mut inputs = Vec::new();
        let mut blocked = Vec::new();
        for from in t.inputs_of(&step.name) {
            let Some(o) = outs.get(from) else { continue };
            let from_gap = o.main.is_some_and(|n| self.ir.nodes[n].is_gap());
            if o.distributed && !from_gap {
                blocked.push(format!(
                    "`{from}` distributes its rows round-robin between its outputs, so this step \
                     sees an arbitrary share of them"
                ));
            }
            let node = o.routed.get(&step.name).copied().or(o.main);
            match node {
                Some(n) if self.ir.nodes[n].is_gap() => {
                    blocked.push(format!("input `{from}` is not translated"));
                    inputs.push(Input {
                        from: from.to_string(),
                        node: n,
                    });
                }
                Some(n) => inputs.push(Input {
                    from: from.to_string(),
                    node: n,
                }),
                None => blocked.push(format!("input `{from}` produced nothing")),
            }
        }
        if let Some(&n) = mapping_inputs.get(&step.name) {
            inputs.push(Input {
                from: "mapping input".into(),
                node: n,
            });
        }
        let only_error_inputs = inputs.is_empty()
            && t.hops
                .iter()
                .any(|h| h.to == step.name && h.enabled && h.error);
        if only_error_inputs {
            out.main = Some(self.gap(
                step,
                prefix,
                Vec::new(),
                "the step only receives rows another step rejects (an error hop); a SQL model \
                 fails the query instead of routing bad rows",
            ));
            return out;
        }
        if !blocked.is_empty() {
            let ids = inputs.iter().map(|i| i.node).collect();
            out.main = Some(self.gap(step, prefix, ids, blocked.join("; ")));
            return out;
        }
        let lowered = self.lower_step(t, step, prefix, scope, &inputs);
        let error_note = t
            .hops
            .iter()
            .find(|h| h.from == step.name && h.error && h.enabled)
            .map(|h| {
                format!(
                    "Kettle sends rows this step fails on to `{}`; the SQL fails the run instead",
                    h.to
                )
            });
        match lowered {
            Ok(Lowering::Node(op, ins, schema, mut fid, mut notes)) => {
                if let Some(n) = error_note {
                    notes.push(n);
                    fid = fid.max(Fidelity::Rewritten);
                }
                out.main = Some(self.push(step, prefix, None, op, ins, schema, fid, notes));
            }
            Ok(Lowering::Routed(branches, input, fid, notes)) => {
                let schema = self.ir.nodes[input].schema.clone();
                for (target, pred) in branches {
                    let id = self.push(
                        step,
                        prefix,
                        target.clone(),
                        Op::Filter { predicate: pred },
                        vec![input],
                        schema.clone(),
                        fid,
                        notes.clone(),
                    );
                    match target {
                        Some(t) => {
                            out.routed.insert(t, id);
                        }
                        None => out.main = Some(id),
                    }
                }
            }
            Ok(Lowering::Inlined(id)) => out.main = Some(id),
            Err(gap) => {
                let ids = inputs.iter().map(|i| i.node).collect();
                out.main = Some(self.gap(step, prefix, ids, gap.reason));
            }
        }
        out
    }

    /// The single input of a one-input step; several inputs become a union first (Kettle
    /// interleaves the streams, which must share a layout).
    pub(crate) fn single_input(
        &mut self,
        step: &Step,
        prefix: &str,
        inputs: &[Input],
    ) -> Result<NodeId, Gap> {
        match inputs {
            [] => Err(Gap::new("the step has no input")),
            [one] => Ok(one.node),
            many => {
                let first = self.ir.nodes[many[0].node].schema.clone();
                for i in &many[1..] {
                    let s = &self.ir.nodes[i.node].schema;
                    let names =
                        |s: &[Field]| s.iter().map(|f| f.name.to_lowercase()).collect::<Vec<_>>();
                    if names(s) != names(&first) {
                        return Err(Gap::new(format!(
                            "inputs `{}` and `{}` have different layouts; Kettle requires the same",
                            many[0].from, i.from
                        )));
                    }
                }
                let ids = many.iter().map(|i| i.node).collect();
                let mut synthetic = step.clone();
                synthetic.name = format!("{} (inputs)", step.name);
                synthetic.type_id = "Append".into();
                Ok(self.push(
                    &synthetic,
                    prefix,
                    None,
                    Op::Union,
                    ids,
                    first,
                    Fidelity::Exact,
                    vec!["several hops into one step: their rows are combined".into()],
                ))
            }
        }
    }

    /// Why a step of this category is not translated.
    pub(crate) fn category_gap(step: &Step) -> Gap {
        let what = ekos_lakehouse_kettle::catalogue::readable(&step.type_id);
        Gap::new(match step.category {
            Category::File => {
                format!("{what} reads or writes files, which are outside the lakehouse")
            }
            Category::Orchestration => {
                format!("{what} is run-time plumbing, not a data transformation")
            }
            Category::SubTransformation => {
                format!("{what} runs another transformation per row or row group")
            }
            Category::Code => format!("{what} is not translated"),
            Category::Unknown => format!(
                "compiled plugin step `{}`: decompiling it needs the ekos-binary build (RFC 0203)",
                step.type_id
            ),
            _ => format!("{what} is not translated yet"),
        })
    }

    pub(crate) fn depth_ok(&self) -> bool {
        self.depth < MAX_MAPPING_DEPTH
    }
}

//! EKOS RFC 0267's exit on real projects: every job of the Talend fixtures lowered to the shared
//! relational IR, with coverage by component type and by gap reason.
//!
//! Needs `EKOS_TALEND_FIXTURE_B` … `_E` (EKOS `scripts/talend-fixtures.sh`); each is skipped,
//! saying so, when unset. Third-party projects, never vendored.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ekos_lakehouse_etl_rel::{NoCatalog, Op};
use ekos_lakehouse_talend_rel::lower_job;
use ekos_talend::{PROJECT_MARKER, Project, ProjectFile, Role, read_project, role};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !ekos_talend::SKIPPED_DIRS.contains(&name.as_str()) {
                walk(&p, out);
            }
        } else {
            out.push(p);
        }
    }
}

fn projects(root: &Path) -> Vec<Project> {
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut out = Vec::new();
    for marker in files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == PROJECT_MARKER))
    {
        let dir = marker.parent().unwrap();
        let mut inside = Vec::new();
        walk(dir, &mut inside);
        let pf: Vec<ProjectFile> = inside
            .iter()
            .filter_map(|p| {
                let rel = p
                    .strip_prefix(dir)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                (role(&rel) != Role::Other).then_some(())?;
                Some(ProjectFile {
                    text: std::fs::read_to_string(p).ok()?,
                    path: rel,
                })
            })
            .collect();
        let rel = dir
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        out.push(read_project(&rel, &pf));
    }
    out
}

#[derive(Default)]
struct Coverage {
    data: usize,
    lowered: usize,
    gaps: BTreeMap<String, usize>,
    by_type: BTreeMap<String, (usize, usize)>,
    expressions: usize,
    expression_gaps: BTreeMap<String, usize>,
}

fn cover(p: &Project, cov: &mut Coverage) -> Vec<(String, ekos_lakehouse_etl_rel::Ir)> {
    let mut irs = Vec::new();
    for job in p.jobs.iter().filter(|j| j.latest) {
        let expanded = p.expand(job).job;
        let l = lower_job(&expanded, &NoCatalog);
        let mut per: BTreeMap<&str, (String, bool)> = BTreeMap::new();
        for n in &l.ir.nodes {
            let e = per
                .entry(n.step.as_str())
                .or_insert((n.step_type.clone(), false));
            if n.is_gap() {
                e.1 = true;
                let key: String = match &n.op {
                    Op::Gap { reason } => reason.chars().take(70).collect(),
                    _ => unreachable!(),
                };
                *cov.gaps.entry(key).or_default() += 1;
            }
        }
        for (_, (ty, gap)) in per {
            cov.data += 1;
            let t = cov.by_type.entry(ty).or_default();
            t.0 += 1;
            if !gap {
                cov.lowered += 1;
                t.1 += 1;
            }
        }
        cov.expressions += l.expressions;
        for (k, v) in l.expression_gaps {
            *cov.expression_gaps.entry(k).or_default() += v;
        }
        irs.push((job.name.clone(), l.ir));
    }
    irs
}

fn report(name: &str, cov: &Coverage) {
    eprintln!(
        "{name}: data components {} lowered {} ({:.0}%), expressions {} lowered + {} gaps",
        cov.data,
        cov.lowered,
        100.0 * cov.lowered as f64 / cov.data.max(1) as f64,
        cov.expressions,
        cov.expression_gaps.values().sum::<usize>()
    );
    for (t, (n, ok)) in &cov.by_type {
        eprintln!("  type {t}: {ok}/{n}");
    }
    let mut gaps: Vec<_> = cov.gaps.iter().collect();
    gaps.sort_by(|a, b| b.1.cmp(a.1));
    for (g, n) in gaps.iter().take(12) {
        eprintln!("  gap {n} × {g}");
    }
    let mut eg: Vec<_> = cov.expression_gaps.iter().collect();
    eg.sort_by(|a, b| b.1.cmp(a.1));
    for (g, n) in eg.iter().take(12) {
        eprintln!("  expr gap {n} × {g}");
    }
}

fn fixture(var: &str) -> Option<PathBuf> {
    let p = std::env::var_os(var).map(PathBuf::from);
    if p.is_none() {
        eprintln!("skipped: {var} is not set (EKOS scripts/talend-fixtures.sh)");
    }
    p
}

#[test]
fn fixture_b_dwh_maps_lower_and_the_sales_measure_reads_its_columns() {
    let Some(root) = fixture("EKOS_TALEND_FIXTURE_B") else {
        return;
    };
    let all = projects(&root);
    let p = all
        .iter()
        .find(|p| p.root.ends_with("ALIMENTATION_DATA_WAREHOUSE"))
        .unwrap();
    let mut cov = Coverage::default();
    let irs = cover(p, &mut cov);
    report("B", &cov);
    let (_, vente) = irs.iter().find(|(n, _)| n == "jDwhVente").unwrap();
    // NB_MT_VENTE is written from the aggregate's sum, whose subject a map computes as
    // QTE_VENTE * PRIX_VENTE.
    let item = |n: &ekos_lakehouse_etl_rel::RelNode, name: &str| match &n.op {
        Op::Project { items } => items
            .iter()
            .find(|(i, _)| i == name)
            .map(|(_, e)| e.clone()),
        _ => None,
    };
    let written = vente
        .nodes
        .iter()
        .find_map(|n| item(n, "NB_MT_VENTE"))
        .expect("NB_MT_VENTE is written");
    let from = written.columns();
    let (agg_node, agg) = vente
        .nodes
        .iter()
        .find_map(|n| match &n.op {
            Op::Aggregate { aggs, .. } => aggs
                .iter()
                .find(|a| from.iter().any(|c| c.ends_with(&a.name)))
                .map(|a| (n, a)),
            _ => None,
        })
        .expect("the aggregate behind NB_MT_VENTE");
    assert_eq!(agg.func, "sum");
    let subject = agg.subject.clone().unwrap();
    let computed = vente
        .upstream(agg_node.id)
        .into_iter()
        .find_map(|i| item(&vente.nodes[i], &subject))
        .expect("the summed column is computed");
    let sql = computed.sql();
    assert!(
        sql.contains("QTE_VENTE") && sql.contains("PRIX_VENTE"),
        "{sql}"
    );
    for (job, ir) in irs.iter().filter(|(n, _)| n.starts_with("jDwh")) {
        for n in ir.nodes.iter().filter(|n| n.step_type == "tMap") {
            assert!(!n.is_gap(), "{job}.{}: {:?}", n.step, n.op);
        }
    }
}

#[test]
fn fixtures_c_d_e_coverage() {
    for (name, var) in [
        ("C", "EKOS_TALEND_FIXTURE_C"),
        ("D", "EKOS_TALEND_FIXTURE_D"),
        ("E", "EKOS_TALEND_FIXTURE_E"),
    ] {
        let Some(root) = fixture(var) else {
            continue;
        };
        let mut cov = Coverage::default();
        for p in projects(&root) {
            cover(&p, &mut cov);
        }
        report(name, &cov);
        assert!(cov.data > 0);
    }
}

#[test]
#[ignore = "debug listing"]
fn list_jdwhvente() {
    let root = PathBuf::from(std::env::var("EKOS_TALEND_FIXTURE_B").unwrap());
    let all = projects(&root);
    let p = all
        .iter()
        .find(|p| p.root.ends_with("ALIMENTATION_DATA_WAREHOUSE"))
        .unwrap();
    let job = p.jobs.iter().find(|j| j.name == "jDwhVente").unwrap();
    let l = lower_job(&p.expand(job).job, &NoCatalog);
    for n in &l.ir.nodes {
        let what = match &n.op {
            Op::Gap { reason } => format!("GAP {reason}"),
            Op::Aggregate { group_by, aggs } => format!(
                "AGG {group_by:?} {:?}",
                aggs.iter()
                    .map(|a| (&a.name, &a.func, &a.subject))
                    .collect::<Vec<_>>()
            ),
            other => format!("{:?}", std::mem::discriminant(other)),
        };
        eprintln!(
            "{} {} {:?} in={:?} {}",
            n.id,
            n.step,
            n.branch,
            n.inputs,
            what.chars().take(160).collect::<String>()
        );
    }
}

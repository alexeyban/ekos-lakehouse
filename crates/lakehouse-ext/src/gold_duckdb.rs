//! EKOS RFC 0259 L3/L5: publishing gold marts built on DuckDB. Every mart is built by dbt-duckdb
//! into `marts.next.duckdb` beside the published `marts.duckdb`, its tests run, each additive
//! measure is reconciled with silver in Trino (both sides in RFC 0155's canonical form), and only
//! then is the new file renamed over the published one. A failure at any step leaves the
//! published file as it was and says which step failed and why.
//!
//! The steps are a trait ([`Steps`]) so the failure paths are tested without dbt, DuckDB or Trino;
//! [`LiveSteps`] runs them for real: `dbt build` with the file's path in
//! [`ekos_lakehouse_gold::duckdb::NEXT_ENV`], the DuckDB side through Python's `duckdb` module on a
//! read-only connection with external access off, the Trino side through the read-only reader.

use crate::facts::RunReport;
use crate::marts::Totals;
use ekos_lakehouse_gold::duckdb::{CrossCheck, NEXT_ENV, PROJECT_DIR, agrees, cross_checks};
use ekos_lakehouse_gold::{Compiled, MartSpec, Resolved};
use std::path::{Path, PathBuf};

/// The published marts file and the file a build writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub published: PathBuf,
    pub next: PathBuf,
}

impl Paths {
    /// `…/marts.duckdb` → `…/marts.next.duckdb`, in the same directory, so the rename is atomic.
    pub fn of(published: &Path) -> Self {
        let stem = published
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "marts".into());
        Self {
            published: published.to_path_buf(),
            next: published.with_file_name(format!("{stem}.next.duckdb")),
        }
    }
}

/// The default published file, relative to the pipeline repository (git-ignored).
pub const DEFAULT_MARTS_FILE: &str = ".lakehouse/marts/marts.duckdb";

/// What a build reported: one run per model (dbt's results), and whether every model built and
/// every test passed.
#[derive(Debug, Clone, Default)]
pub struct Build {
    pub reports: Vec<RunReport>,
    pub ok: bool,
}

/// One mart's measures, each with both totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MartTotals {
    pub mart: String,
    pub totals: Vec<Totals>,
    /// The mart's row count in the built file (dbt-duckdb reports none for a table).
    pub rows: Option<i64>,
}

pub trait Steps {
    /// Build every mart into `next` and run its tests.
    fn build(&mut self, next: &Path) -> anyhow::Result<Build>;
    /// Each mart's measures in `next` against silver.
    fn reconcile(&mut self, next: &Path) -> anyhow::Result<Vec<MartTotals>>;
}

/// What a publication did. `refused` says why the published file was left as it was.
#[derive(Debug, Clone, Default)]
pub struct Publication {
    pub published: bool,
    /// `(step, why)`: `build`, `tests` or `reconcile`.
    pub refused: Option<(&'static str, String)>,
    /// One run per mart, as recorded for health and freshness (RFC 0196/0258).
    pub reports: Vec<RunReport>,
    pub reconciliations: Vec<MartTotals>,
    /// The published file's size in bytes, when it was published.
    pub bytes: Option<u64>,
}

fn remove_if_there(p: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!("{}: {e}", p.display())),
    }
}

fn wal(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".wal");
    PathBuf::from(s)
}

/// Mark every gold run failed with `why`, keeping a model's own failure first.
fn fail_all(reports: &mut [RunReport], why: &str) {
    for r in reports.iter_mut().filter(|r| r.layer == "gold") {
        r.status = "failed".into();
        r.message = Some(match r.message.take() {
            Some(m) => format!("{m}; {why}"),
            None => why.to_string(),
        });
    }
}

/// Build, test, reconcile, publish. Never touches `paths.published` unless every step passed.
pub fn publish(paths: &Paths, steps: &mut dyn Steps) -> Publication {
    let mut out = Publication::default();
    let refuse = |mut out: Publication, step: &'static str, why: String| {
        fail_all(&mut out.reports, &format!("not published: {why}"));
        out.refused = Some((step, why));
        out
    };
    // A build always starts from an empty file: nothing of an earlier, refused build survives.
    if let Err(e) = remove_if_there(&paths.next).and_then(|_| remove_if_there(&wal(&paths.next))) {
        return refuse(out, "build", e.to_string());
    }
    if let Some(dir) = paths.next.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        return refuse(out, "build", format!("{}: {e}", dir.display()));
    }
    let build = match steps.build(&paths.next) {
        Ok(b) => b,
        Err(e) => return refuse(out, "build", format!("{e:#}")),
    };
    out.reports = build.reports;
    if !build.ok {
        let failing: Vec<String> = out
            .reports
            .iter()
            .filter(|r| r.status != "ok")
            .map(|r| match &r.message {
                Some(m) => format!("{} ({m})", r.table),
                None => r.table.clone(),
            })
            .collect();
        let why = if failing.is_empty() {
            "dbt build failed".to_string()
        } else {
            format!("dbt build failed: {}", failing.join(", "))
        };
        return refuse(out, "tests", why);
    }
    if !paths.next.is_file() {
        return refuse(
            out,
            "build",
            format!("the build wrote no {}", paths.next.display()),
        );
    }
    if wal(&paths.next).exists() {
        // dbt-duckdb closes its connection, which checkpoints; a log left behind means the file
        // alone is not the whole build.
        return refuse(
            out,
            "build",
            format!(
                "{} has an uncheckpointed write-ahead log",
                paths.next.display()
            ),
        );
    }
    let recs = match steps.reconcile(&paths.next) {
        Ok(r) => r,
        Err(e) => return refuse(out, "reconcile", format!("{e:#}")),
    };
    out.reconciliations = recs;
    for m in &out.reconciliations {
        if let Some(r) = out
            .reports
            .iter_mut()
            .find(|r| r.layer == "gold" && r.table == format!("gold.{}", m.mart))
            && r.rows.is_none()
        {
            r.rows = m.rows;
        }
    }
    let differing: Vec<String> = out
        .reconciliations
        .iter()
        .flat_map(|m| {
            m.totals.iter().filter(|t| !t.equal).map(move |t| {
                format!(
                    "{}.{}: mart {} / silver {}",
                    m.mart, t.measure, t.mart_total, t.silver_total
                )
            })
        })
        .collect();
    if !differing.is_empty() {
        let why = format!("does not reconcile with silver: {}", differing.join("; "));
        return refuse(out, "reconcile", why);
    }
    // The publication itself: one rename within a directory, so a reader sees the old file or the
    // new one, never a part of either.
    if let Err(e) = std::fs::rename(&paths.next, &paths.published) {
        return refuse(
            out,
            "publish",
            format!(
                "{} → {}: {e}",
                paths.next.display(),
                paths.published.display()
            ),
        );
    }
    out.published = true;
    out.bytes = std::fs::metadata(&paths.published).ok().map(|m| m.len());
    out
}

// ─────────────────────────────── the live steps ───────────────────────────────

/// One compiled mart, as `lakehouse gold publish` reconciles it.
pub struct MartToCheck {
    pub spec: MartSpec,
    pub resolved: Vec<Resolved>,
    pub compiled: Compiled,
}

/// Reads a DuckDB file read-only: Python's `duckdb` module, external access off before the first
/// statement, so a query can reach neither files nor the network. Queries on stdin as a JSON
/// list; each query's first row back as text (`null` for NULL).
pub struct DuckDbRunner {
    pub python: String,
}

const READ_SCRIPT: &str = r#"
import json, sys
import duckdb
con = duckdb.connect(sys.argv[1], read_only=True, config={
    "enable_external_access": "false",
    "autoinstall_known_extensions": "false",
    "autoload_known_extensions": "false",
})
con.execute("SET lock_configuration = true")
out = []
for q in json.load(sys.stdin):
    row = con.execute(q).fetchone()
    out.append([] if row is None else [None if v is None else str(v) for v in row])
con.close()
print(json.dumps(out))
"#;

impl DuckDbRunner {
    /// The first row of each query, every value as text.
    pub fn first_rows(
        &self,
        file: &Path,
        queries: &[String],
    ) -> anyhow::Result<Vec<Vec<Option<String>>>> {
        use std::io::Write;
        let mut child = std::process::Command::new(&self.python)
            .arg("-I")
            .arg("-c")
            .arg(READ_SCRIPT)
            .arg(file)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("starting {} for DuckDB: {e}", self.python))?;
        child
            .stdin
            .take()
            .expect("piped")
            .write_all(serde_json::to_string(queries)?.as_bytes())?;
        let out = child.wait_with_output()?;
        anyhow::ensure!(
            out.status.success(),
            "reading {} with DuckDB: {}",
            file.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(serde_json::from_slice(&out.stdout)?)
    }
}

/// dbt, DuckDB and Trino, as `lakehouse gold publish` runs them.
pub struct LiveSteps {
    /// The pipeline repository.
    pub repo: PathBuf,
    pub dbt: String,
    pub duckdb: DuckDbRunner,
    pub trino: ekos_lakehouse_trino::Connection,
    pub plan: ekos_lakehouse_bronze::Plan,
    pub marts: Vec<MartToCheck>,
    /// dbt's target directory for this run (results are read from it).
    pub target: PathBuf,
}

impl Steps for LiveSteps {
    fn build(&mut self, next: &Path) -> anyhow::Result<Build> {
        let project = self.repo.join(PROJECT_DIR);
        let next = std::path::absolute(next)?;
        let out = std::process::Command::new(&self.dbt)
            .arg("build")
            .arg("--project-dir")
            .arg(&project)
            .arg("--profiles-dir")
            .arg(&project)
            .arg("--target-path")
            .arg(&self.target)
            .current_dir(&project)
            .env(NEXT_ENV, &next)
            .output()
            .map_err(|e| anyhow::anyhow!("starting {}: {e}", self.dbt))?;
        let log = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        let reports = match crate::monitor::dbt_results(&self.target) {
            Ok(r) => r,
            Err(_) => anyhow::bail!(
                "dbt build did not run: {}",
                log.lines()
                    .rev()
                    .take(12)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        };
        let ok = out.status.success() && reports.iter().all(|r| r.status == "ok");
        Ok(Build { reports, ok })
    }

    fn reconcile(&mut self, next: &Path) -> anyhow::Result<Vec<MartTotals>> {
        let checks: Vec<(String, Vec<CrossCheck>)> = self
            .marts
            .iter()
            .map(|m| {
                (
                    m.spec.mart.clone(),
                    cross_checks(
                        &m.spec,
                        &m.resolved,
                        &m.compiled,
                        &self.plan,
                        &self.plan.settings.target_catalog,
                    ),
                )
            })
            .collect();
        let gold_schema =
            ekos_lakehouse_bronze::plan::gold_schema(&self.plan.settings.namespace_prefix);
        let ident = ekos_lakehouse_bronze::typemap::ident;
        // Per mart: its row count, then each measure's total.
        let duck_queries: Vec<String> = checks
            .iter()
            .flat_map(|(mart, cs)| {
                std::iter::once(format!(
                    "SELECT count(*) FROM {}.{}",
                    ident(&gold_schema),
                    ident(mart)
                ))
                .chain(cs.iter().map(|c| c.mart_sql.clone()))
            })
            .collect();
        let duck = self.duckdb.first_rows(next, &duck_queries)?;
        let trino = ekos_lakehouse_trino::TrinoReader::new(self.trino.clone())?;
        let mut answers = duck.into_iter().map(|row| row.into_iter().next().flatten());
        let mut out = vec![];
        for (mart, cs) in checks {
            let rows = answers.next().flatten().and_then(|n| n.parse().ok());
            let mut totals = vec![];
            for c in cs {
                let mart_total = answers.next().flatten().unwrap_or_default();
                let silver_total = trino
                    .read(&c.silver_sql)?
                    .into_iter()
                    .next()
                    .and_then(|row| row.into_iter().next())
                    .unwrap_or_default();
                totals.push(Totals {
                    equal: agrees(&c, &mart_total, &silver_total),
                    measure: c.measure,
                    mart_total,
                    silver_total,
                });
            }
            out.push(MartTotals { mart, totals, rows });
        }
        Ok(out)
    }
}

/// The marts the dbt-duckdb project holds: `gold__<mart>.sql` under `models/gold`.
pub fn marts_in(repo: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(repo.join(PROJECT_DIR).join("models/gold"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix("gold__")
                .and_then(|n| n.strip_suffix(".sql"))
                .map(str::to_string)
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ekos-lh-duckpub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn run(table: &str, status: &str) -> RunReport {
        RunReport {
            table: table.into(),
            layer: "gold".into(),
            run_id: String::new(),
            status: status.into(),
            rows: Some(3),
            duration_ms: None,
            snapshot_id: None,
            message: None,
        }
    }

    /// A fake build: writes `bytes` into the next file, reports `status` for the mart; a fake
    /// reconciliation answers `mart` / `silver`.
    struct Fake {
        build: Result<(&'static str, &'static [u8]), &'static str>,
        totals: Result<(&'static str, &'static str), &'static str>,
    }

    impl Steps for Fake {
        fn build(&mut self, next: &Path) -> anyhow::Result<Build> {
            let (status, bytes) = self.build.map_err(|e| anyhow::anyhow!(e))?;
            std::fs::write(next, bytes)?;
            Ok(Build {
                reports: vec![run("gold.sales", status)],
                ok: status == "ok",
            })
        }
        fn reconcile(&mut self, _: &Path) -> anyhow::Result<Vec<MartTotals>> {
            let (m, s) = self.totals.map_err(|e| anyhow::anyhow!(e))?;
            Ok(vec![MartTotals {
                rows: Some(3),
                mart: "sales".into(),
                totals: vec![Totals {
                    measure: "amount".into(),
                    mart_total: m.into(),
                    silver_total: s.into(),
                    equal: m == s,
                }],
            }])
        }
    }

    fn published(dir: &Path) -> Paths {
        let p = Paths::of(&dir.join("marts.duckdb"));
        std::fs::write(&p.published, b"the published marts").unwrap();
        p
    }

    #[test]
    fn the_next_file_sits_beside_the_published_one() {
        let p = Paths::of(Path::new("/x/.lakehouse/marts/marts.duckdb"));
        assert_eq!(p.next, Path::new("/x/.lakehouse/marts/marts.next.duckdb"));
    }

    #[test]
    fn a_passing_build_and_reconciliation_replace_the_published_file() {
        let d = tmp("ok");
        let p = published(&d);
        let r = publish(
            &p,
            &mut Fake {
                build: Ok(("ok", b"new marts")),
                totals: Ok(("10.00", "10.00")),
            },
        );
        assert!(r.published && r.refused.is_none(), "{r:?}");
        assert_eq!(std::fs::read(&p.published).unwrap(), b"new marts");
        assert!(!p.next.exists());
        assert_eq!(r.bytes, Some(9));
        assert_eq!(r.reports[0].status, "ok");
    }

    /// Every failure leaves the published file byte for byte, says which step failed and why,
    /// and marks the marts' runs failed.
    #[test]
    fn any_failure_leaves_the_published_file_untouched() {
        let cases: [(Fake, &str, &str); 4] = [
            (
                Fake {
                    build: Err("dbt build did not run: profile not found"),
                    totals: Ok(("1", "1")),
                },
                "build",
                "profile not found",
            ),
            (
                Fake {
                    build: Ok(("failed", b"half built")),
                    totals: Ok(("1", "1")),
                },
                "tests",
                "gold.sales",
            ),
            (
                Fake {
                    build: Ok(("ok", b"new marts")),
                    totals: Ok(("10.01", "10.00")),
                },
                "reconcile",
                "sales.amount: mart 10.01 / silver 10.00",
            ),
            (
                Fake {
                    build: Ok(("ok", b"new marts")),
                    totals: Err("Trino is not answering"),
                },
                "reconcile",
                "Trino is not answering",
            ),
        ];
        for (i, (mut fake, step, why)) in cases.into_iter().enumerate() {
            let d = tmp(&format!("fail{i}"));
            let p = published(&d);
            let r = publish(&p, &mut fake);
            assert!(!r.published, "{i}");
            let (s, w) = r.refused.clone().unwrap();
            assert_eq!(s, step, "{i}: {w}");
            assert!(w.contains(why), "{i}: {w}");
            assert_eq!(
                std::fs::read(&p.published).unwrap(),
                b"the published marts",
                "{i}"
            );
            assert!(
                r.reports.iter().all(|x| x.status == "failed"),
                "{i}: {:?}",
                r.reports
            );
        }
    }

    /// A refused build is never the start of the next one: the next file is rebuilt from nothing.
    #[test]
    fn a_stale_next_file_is_removed_before_a_build() {
        let d = tmp("stale");
        let p = published(&d);
        std::fs::write(&p.next, b"left over").unwrap();
        std::fs::write(wal(&p.next), b"left over log").unwrap();
        struct Sees(Option<bool>);
        impl Steps for Sees {
            fn build(&mut self, next: &Path) -> anyhow::Result<Build> {
                self.0 = Some(next.exists() || wal(next).exists());
                anyhow::bail!("stop here")
            }
            fn reconcile(&mut self, _: &Path) -> anyhow::Result<Vec<MartTotals>> {
                unreachable!()
            }
        }
        let mut s = Sees(None);
        let r = publish(&p, &mut s);
        assert_eq!(s.0, Some(false));
        assert!(!r.published);
    }

    #[test]
    fn the_marts_are_the_projects_gold_models() {
        let d = tmp("list");
        let g = d.join("gold-duckdb/models/gold");
        std::fs::create_dir_all(&g).unwrap();
        for f in ["gold__b.sql", "gold__a.sql", "_gold__a.yml", "other.sql"] {
            std::fs::write(g.join(f), "").unwrap();
        }
        assert_eq!(marts_in(&d), ["a", "b"]);
    }
}

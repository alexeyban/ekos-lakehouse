//! EKOS RFC 0227 §2.4: `lakehouse run` — launch the pipeline's Dagster job through Dagster's
//! GraphQL API, and optionally follow it to the end. EKOS still moves no row: Dagster runs the
//! job, which sends the generated SQL to Trino.

use serde_json::{Value, json};
use std::time::Duration;

const REPOSITORIES: &str = "query { repositoriesOrError { __typename \
    ... on RepositoryConnection { nodes { name location { name } jobs { name } \
    assetNodes { assetKey { path } } } } \
    ... on PythonError { message } } }";

const LAUNCH: &str = "mutation Launch($p: ExecutionParams!) { launchRun(executionParams: $p) { \
    __typename \
    ... on LaunchRunSuccess { run { runId } } \
    ... on PythonError { message } \
    ... on RunConfigValidationInvalid { errors { message } } \
    ... on PipelineNotFoundError { message } \
    ... on InvalidSubsetError { message } \
    ... on UnauthorizedError { message } \
    ... on RunConflict { message } } }";

const RELOAD: &str = "mutation Reload($l: String!) { reloadRepositoryLocation(repositoryLocationName: $l) { \
    __typename ... on WorkspaceLocationEntry { loadStatus } ... on PythonError { message } \
    ... on RepositoryLocationNotFound { message } ... on UnauthorizedError { message } } }";

const STATUS: &str = "query Status($id: ID!) { runOrError(runId: $id) { __typename \
    ... on Run { status } ... on RunNotFoundError { message } ... on PythonError { message } } }";

/// Where a job lives in Dagster's workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRef {
    pub location: String,
    pub repository: String,
    pub job: String,
}

/// Pure: the one repository that has `job` (in `location`, when given), from the repositories
/// query's answer.
pub fn find_job(answer: &Value, job: &str, location: Option<&str>) -> anyhow::Result<JobRef> {
    let r = &answer["data"]["repositoriesOrError"];
    if r["__typename"] != "RepositoryConnection" {
        anyhow::bail!(
            "Dagster could not list its repositories: {}",
            r["message"].as_str().unwrap_or("no answer")
        );
    }
    let found: Vec<JobRef> = r["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| {
            n["jobs"]
                .as_array()
                .is_some_and(|j| j.iter().any(|x| x["name"] == job))
        })
        .map(|n| JobRef {
            location: n["location"]["name"].as_str().unwrap_or_default().into(),
            repository: n["name"].as_str().unwrap_or_default().into(),
            job: job.into(),
        })
        .filter(|j| location.is_none_or(|l| j.location == l))
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => anyhow::bail!(
            "no code location in Dagster has a job `{job}`{}: is this project's pipeline loaded?",
            location.map(|l| format!(" in {l}")).unwrap_or_default()
        ),
        many => anyhow::bail!(
            "several code locations have a job `{job}` ({}): pass --location",
            many.iter()
                .map(|j| j.location.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Pure: the key of the asset named `name` (its last path part, e.g. a dbt model `gold__sales`)
/// in the job's repository (EKOS RFC 0228). None when there is none, or several.
pub fn find_asset(answer: &Value, j: &JobRef, name: &str) -> Option<Vec<String>> {
    let nodes = answer["data"]["repositoriesOrError"]["nodes"].as_array()?;
    let repo = nodes.iter().find(|n| {
        n["name"] == j.repository.as_str() && n["location"]["name"] == j.location.as_str()
    })?;
    let keys: Vec<Vec<String>> = repo["assetNodes"]
        .as_array()?
        .iter()
        .filter_map(|a| {
            let path: Vec<String> = a["assetKey"]["path"]
                .as_array()?
                .iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect();
            (path.last().map(String::as_str) == Some(name)).then_some(path)
        })
        .collect();
    match keys.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// Pure: the launch mutation's variables. `in_process` runs every step in the run's own process,
/// one after another: no process per step, which on a pipeline of hundreds of tables costs more
/// than the steps themselves.
pub fn launch_variables(j: &JobRef, in_process: bool, assets: &[Vec<String>]) -> Value {
    let mut p = json!({"selector": {
        "repositoryLocationName": j.location,
        "repositoryName": j.repository,
        "jobName": j.job,
    }});
    // EKOS RFC 0228: only these assets (a mart), not the whole job.
    if !assets.is_empty() {
        p["selector"]["assetSelection"] = assets.iter().map(|k| json!({"path": k})).collect();
    }
    if in_process {
        p["runConfigData"] = json!({"execution": {"config": {"in_process": {}}}});
    }
    json!({ "p": p })
}

/// Pure: the run id, or why Dagster refused.
pub fn launched(answer: &Value) -> anyhow::Result<String> {
    let r = &answer["data"]["launchRun"];
    if let Some(id) = r["run"]["runId"].as_str() {
        return Ok(id.to_string());
    }
    let mut why: Vec<String> = r["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["message"].as_str().map(str::to_string))
        .collect();
    if let Some(m) = r["message"].as_str() {
        why.push(m.to_string());
    }
    if let Some(e) = answer["errors"].as_array() {
        why.extend(
            e.iter()
                .filter_map(|e| e["message"].as_str().map(str::to_string)),
        );
    }
    anyhow::bail!(
        "Dagster did not launch the run ({}): {}",
        r["__typename"].as_str().unwrap_or("no answer"),
        why.join("; ")
    )
}

/// Pure: a run's status, when it has one.
pub fn status(answer: &Value) -> Option<String> {
    answer["data"]["runOrError"]["status"]
        .as_str()
        .map(str::to_string)
}

pub const TERMINAL: &[&str] = &["SUCCESS", "FAILURE", "CANCELED"];

fn post(base: &str, query: &str, variables: Value) -> anyhow::Result<Value> {
    let client = reqwest::blocking::Client::builder()
        // Launching a job of hundreds of assets plans the whole run first: minutes, not seconds.
        .timeout(Duration::from_secs(600))
        .build()?;
    let r = client
        .post(format!("{}/graphql", base.trim_end_matches('/')))
        .json(&json!({"query": query, "variables": variables}))
        .send()?;
    if !r.status().is_success() {
        anyhow::bail!("Dagster answered {}", r.status());
    }
    Ok(r.json()?)
}

/// Where the pipeline repository records the commit Dagster last loaded (run-time state).
pub const LOADED_MARKER: &str = ".lakehouse/dagster-loaded";

/// Pure: whether the commit Dagster loaded is not the repository's `HEAD`.
pub fn needs_reload(head: &str, loaded: Option<&str>) -> bool {
    loaded.map(str::trim) != Some(head.trim())
}

/// Pure: a reload mutation's answer, as the load status or the error.
pub fn reloaded(answer: &Value) -> anyhow::Result<String> {
    let t = &answer["data"]["reloadRepositoryLocation"];
    if t["__typename"] != "WorkspaceLocationEntry" {
        anyhow::bail!(
            "Dagster could not reload the code location: {}",
            t["message"].as_str().unwrap_or("no answer")
        );
    }
    Ok(t["loadStatus"].as_str().unwrap_or("LOADED").to_string())
}

/// EKOS RFC 0260 demo, gap 7: Dagster reads the generated definitions when it loads the code
/// location, so after `generate` (or a sync that regenerated) a job it launches still has the
/// old assets — new tables had no bronze asset, and their silver models failed. When the
/// repository's `HEAD` is not the commit last loaded, reload the location that has `job` and wait
/// until it has loaded. `Ok(false)`: nothing to reload.
pub fn reload_if_changed(base: &str, repo: &std::path::Path, job: &str) -> anyhow::Result<bool> {
    let head = crate::sync::git(repo, &["rev-parse", "HEAD"])?;
    let marker = repo.join(LOADED_MARKER);
    let loaded = std::fs::read_to_string(&marker).ok();
    if !needs_reload(&head, loaded.as_deref()) {
        return Ok(false);
    }
    let j = find_job(&post(base, REPOSITORIES, json!({}))?, job, None)?;
    let mut status = reloaded(&post(base, RELOAD, json!({"l": j.location}))?)?;
    let started = std::time::Instant::now();
    while status != "LOADED" {
        if started.elapsed() > Duration::from_secs(600) {
            anyhow::bail!("{} did not finish loading in 10 minutes", j.location);
        }
        std::thread::sleep(Duration::from_secs(5));
        if find_job(
            &post(base, REPOSITORIES, json!({}))?,
            job,
            Some(&j.location),
        )
        .is_ok()
        {
            status = "LOADED".into();
        }
    }
    if let Some(dir) = marker.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&marker, format!("{head}\n"))?;
    Ok(true)
}

/// [`reload_if_changed`] for a command whose own work is done: a failure is a warning, since
/// Dagster may be down or not reachable from where the command runs.
pub fn reload_after(base: Option<&str>, repo: &std::path::Path) {
    let Some(base) = base else { return };
    match reload_if_changed(base, repo, "bronze") {
        Ok(true) => println!("Dagster reloaded the pipeline's code location"),
        Ok(false) => {}
        Err(e) => eprintln!(
            "warning: Dagster was not reloaded ({e}); reload the code location before the next \
             run, or new tables have no assets"
        ),
    }
}

/// Launch `job`, print where to follow it, and with `wait` poll until it ends. Blocking: the
/// caller runs it off the async runtime.
pub fn run(
    base: &str,
    job: &str,
    location: Option<&str>,
    in_process: bool,
    wait: bool,
    asset: Option<&str>,
) -> anyhow::Result<String> {
    let repos = post(base, REPOSITORIES, json!({}))?;
    let j = find_job(&repos, job, location)?;
    let assets = match asset {
        Some(a) => match find_asset(&repos, &j, a) {
            Some(k) => vec![k],
            // A mart merged since Dagster loaded the code: reload the location, then look again
            // (EKOS RFC 0228; loading a large project, its dbt manifest included, takes minutes).
            None => {
                let r = post(base, RELOAD, json!({"l": j.location}))?;
                let t = &r["data"]["reloadRepositoryLocation"];
                if t["__typename"] != "WorkspaceLocationEntry" {
                    anyhow::bail!(
                        "Dagster could not reload {}: {}",
                        j.location,
                        t["message"].as_str().unwrap_or("no answer")
                    );
                }
                let started = std::time::Instant::now();
                loop {
                    if let Some(k) = find_asset(&post(base, REPOSITORIES, json!({}))?, &j, a) {
                        break vec![k];
                    }
                    if started.elapsed() > Duration::from_secs(600) {
                        anyhow::bail!(
                            "no single asset `{a}` in {} after reloading it: is the mart merged and \
                             the pipeline generated?",
                            j.location
                        );
                    }
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        },
        None => vec![],
    };
    let id = launched(&post(
        base,
        LAUNCH,
        launch_variables(&j, in_process, &assets),
    )?)?;
    println!(
        "launched {} in {} as run {id}: {}/runs/{id}",
        j.job,
        j.location,
        base.trim_end_matches('/')
    );
    if !wait {
        return Ok(id);
    }
    let started = std::time::Instant::now();
    let mut last = String::new();
    loop {
        std::thread::sleep(Duration::from_secs(10));
        let Some(s) = status(&post(base, STATUS, json!({"id": id}))?) else {
            anyhow::bail!("Dagster no longer knows run {id}");
        };
        if s != last {
            println!("{:>6}s  {s}", started.elapsed().as_secs());
            last = s.clone();
        }
        if TERMINAL.contains(&s.as_str()) {
            return if s == "SUCCESS" {
                Ok(id)
            } else {
                anyhow::bail!("run {id} ended {s}: see its page for the failed steps")
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repos() -> Value {
        json!({"data": {"repositoriesOrError": {"__typename": "RepositoryConnection", "nodes": [
            {"name": "__repository__", "location": {"name": "idem"}, "jobs": [{"name": "bronze"}, {"name": "bronze_maintenance_job"}]},
            {"name": "__repository__", "location": {"name": "pagila"}, "jobs": [{"name": "bronze"}]},
            {"name": "__repository__", "location": {"name": "other"}, "jobs": [{"name": "x"}]}
        ]}}})
    }

    #[test]
    fn the_job_is_found_by_name_and_location() {
        let e = find_job(&repos(), "bronze", None).unwrap_err().to_string();
        assert!(e.contains("idem, pagila"), "{e}");
        let j = find_job(&repos(), "bronze", Some("idem")).unwrap();
        assert_eq!(j.repository, "__repository__");
        assert_eq!(
            launch_variables(&j, false, &[])["p"]["selector"]["repositoryLocationName"],
            "idem"
        );
        assert!(
            launch_variables(&j, false, &[])["p"]
                .get("runConfigData")
                .is_none()
        );
        assert_eq!(
            launch_variables(&j, true, &[])["p"]["runConfigData"]["execution"]["config"],
            json!({"in_process": {}})
        );
        assert!(find_job(&repos(), "nope", None).is_err());
        let broken = json!({"data": {"repositoriesOrError": {"__typename": "PythonError", "message": "boom"}}});
        assert!(
            find_job(&broken, "bronze", None)
                .unwrap_err()
                .to_string()
                .contains("boom")
        );
    }

    #[test]
    fn one_asset_is_found_by_its_name_and_selected() {
        let r = json!({"data": {"repositoriesOrError": {"__typename": "RepositoryConnection", "nodes": [
            {"name": "__repository__", "location": {"name": "idem"}, "jobs": [{"name": "bronze"}],
             "assetNodes": [{"assetKey": {"path": ["gold__sales"]}}, {"assetKey": {"path": ["silver", "ADEMPIERE__C_ORDER"]}}]}
        ]}}});
        let j = find_job(&r, "bronze", None).unwrap();
        let k = find_asset(&r, &j, "gold__sales").unwrap();
        assert_eq!(k, ["gold__sales"]);
        assert!(find_asset(&r, &j, "gold__nope").is_none());
        let v = launch_variables(&j, true, &[k]);
        assert_eq!(
            v["p"]["selector"]["assetSelection"],
            json!([{"path": ["gold__sales"]}])
        );
    }

    #[test]
    fn a_launch_answers_with_a_run_id_or_the_reason() {
        let ok = json!({"data": {"launchRun": {"__typename": "LaunchRunSuccess", "run": {"runId": "r1"}}}});
        // EKOS RFC 0260 demo, gap 7: reload only when HEAD moved since the last load.
        assert!(needs_reload("abc", None));
        assert!(needs_reload("abc", Some("def\n")));
        assert!(!needs_reload("abc", Some("abc\n")));
        let entry = json!({"data": {"reloadRepositoryLocation": {"__typename": "WorkspaceLocationEntry", "loadStatus": "LOADING"}}});
        assert_eq!(reloaded(&entry).unwrap(), "LOADING");
        let missing = json!({"data": {"reloadRepositoryLocation": {"__typename": "RepositoryLocationNotFound", "message": "no x"}}});
        assert!(reloaded(&missing).unwrap_err().to_string().contains("no x"));
        assert_eq!(launched(&ok).unwrap(), "r1");
        let bad = json!({"data": {"launchRun": {"__typename": "RunConfigValidationInvalid", "errors": [{"message": "missing ops"}]}}});
        let e = launched(&bad).unwrap_err().to_string();
        assert!(
            e.contains("RunConfigValidationInvalid") && e.contains("missing ops"),
            "{e}"
        );
        assert_eq!(
            status(&json!({"data": {"runOrError": {"__typename": "Run", "status": "STARTED"}}})),
            Some("STARTED".into())
        );
    }
}

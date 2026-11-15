//! EKOS RFC 0260 §2.4 — `lakehouse spec`: the specification as one Markdown document, written from
//! what the project already holds (requirements and their approvals, the extraction plan and its
//! approval, the schedule, the checks, the review). Nothing in it is typed for it, and nothing says
//! "approved" unless an approval record does. Pure: the command gathers a [`SpecInput`].

use serde::Serialize;
use serde_json::Value;

/// One mart: its prose, its requirement as `requirements show` renders it, and its quality checks.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MartPart {
    pub mart: String,
    pub prose: String,
    /// `requirements::markdown`: the description, the questions, the approval.
    pub requirement: String,
    /// `approved`, `draft`, `questions`, … as the requirement's state says.
    pub state: String,
    pub grain: Vec<String>,
    /// Additive measures (each reconciled with silver on every build).
    pub reconciled: Vec<String>,
}

/// One review subject's sign-off.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Signoff {
    pub subject: String,
    /// `signed`, `stale` or `not signed`.
    pub state: String,
    pub by: Option<String>,
    pub at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SpecInput {
    pub project: String,
    pub generated_at: String,
    pub marts: Vec<MartPart>,
    /// (name, kind) of each `[[sources]]` entry.
    pub sources: Vec<(String, String)>,
    /// (name, kind) of each `[[databases]]` entry. Never a credential or a connect string.
    pub databases: Vec<(String, String)>,
    /// `extraction::show`'s JSON.
    pub extraction: Value,
    pub schedule: Option<String>,
    pub gold_engine: String,
    /// dbt tests generated per layer: (layer, test, count).
    pub tests: Vec<(String, String, usize)>,
    pub review: Vec<Signoff>,
}

/// A cron expression in words, for the few shapes a schedule takes; any other is quoted.
pub fn cron_words(cron: &str) -> String {
    let f: Vec<&str> = cron.split_whitespace().collect();
    match f.as_slice() {
        [m, "*", "*", "*", "*"] if m.parse::<u32>().is_ok() => {
            format!("every hour, at minute {m}")
        }
        [m, "*", "*", "*", "*"] if m.starts_with("*/") => {
            format!("every {} minutes", &m[2..])
        }
        [m, h, "*", "*", "*"] if m.parse::<u32>().is_ok() && h.parse::<u32>().is_ok() => {
            format!("every day at {h:0>2}:{m:0>2}")
        }
        _ => format!("on the cron schedule `{cron}`"),
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// The open items: every requirement not approved, an extraction plan not approved, every review
/// subject not signed or stale.
pub fn open_items(i: &SpecInput) -> Vec<String> {
    let mut out = vec![];
    for m in &i.marts {
        if m.state != "approved" {
            out.push(format!("mart `{}`: requirement {}", m.mart, m.state));
        }
    }
    let x = s(&i.extraction["summary"]["state"]);
    if !x.starts_with("approved") {
        out.push(format!(
            "extraction plan: {}",
            if x.is_empty() { "not proposed" } else { x }
        ));
    }
    for r in &i.review {
        if r.state != "signed" {
            out.push(format!("review `{}`: {}", r.subject, r.state));
        }
    }
    out
}

pub fn render(i: &SpecInput) -> String {
    let mut o = format!(
        "# Specification — {}\n\nGenerated {} by `lakehouse spec` from the project's own records: \
         requirements and their approvals, the extraction plan and its approval, the schedule, the \
         checks and the review. Nothing here is typed for this document.\n\n",
        i.project, i.generated_at
    );

    o.push_str("## 1. Purpose\n\n");
    if i.marts.is_empty() {
        o.push_str("No mart has a requirement yet.\n\n");
    }
    for m in &i.marts {
        let prose = m.prose.trim();
        o.push_str(&format!(
            "- **{}**: {}\n",
            m.mart,
            if prose.is_empty() {
                "(no description)"
            } else {
                prose
            }
        ));
    }
    o.push('\n');

    o.push_str("## 2. Marts\n\nEach term as written, how EKOS resolved it, and the approval.\n\n");
    for m in &i.marts {
        // The requirement's own heading is level 3 already.
        o.push_str(m.requirement.trim_end());
        o.push_str("\n\n");
    }

    o.push_str("## 3. Sources\n\n| Name | Kind |\n|---|---|\n");
    for (n, k) in &i.sources {
        o.push_str(&format!("| {n} | {k} (code and documents) |\n"));
    }
    for (n, k) in &i.databases {
        o.push_str(&format!("| {n} | {k} database (read-only) |\n"));
    }
    o.push('\n');

    o.push_str("## 4. Extraction\n\n");
    let sum = &i.extraction["summary"];
    if i.extraction["proposed"] != true {
        o.push_str("No extraction plan has been proposed (`lakehouse plan`).\n\n");
    } else {
        o.push_str(&format!("State: **{}**", s(&sum["state"])));
        if let Some(by) = sum["approved_by"].as_str() {
            o.push_str(&format!(
                " — approved by {by} at {}",
                s(&sum["approved_at"])
            ));
        }
        o.push_str(&format!(
            ". Digest `{}`. Incremental: {} with deletes every run, {} audited, {} other; full: {}; \
             refused: {}; out of scope: {}; masked columns: {}.\n\n",
            s(&sum["digest"]),
            sum["incremental_every_run"],
            sum["incremental_audited"],
            sum["incremental_other"],
            sum["full"],
            sum["refused"],
            sum["excluded"],
            sum["masked_columns"],
        ));
        o.push_str(
            "| Table | Load | Watermark | Deletes | Masked columns |\n|---|---|---|---|---|\n",
        );
        for t in i.extraction["tables"].as_array().into_iter().flatten() {
            o.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                s(&t["table"]),
                s(&t["mode"]),
                t["watermark"].as_str().unwrap_or("—"),
                t["deletes"].as_str().unwrap_or("—"),
                t["masked"]
            ));
        }
        o.push('\n');
    }

    o.push_str("## 5. Schedule\n\n");
    match &i.schedule {
        Some(cron) => o.push_str(&format!(
            "The pipeline runs {} (`{cron}`). Each run: `lakehouse sync` (source changes: safe ones \
             applied, the rest proposed and the table paused) → bronze loads, each followed by its \
             delete audit and reconciliation → silver{} → health and alerts.\n\n",
            cron_words(cron),
            if i.gold_engine == "duckdb" {
                " (dbt-trino) → gold marts built, tested and reconciled in DuckDB, then published"
            } else {
                " and gold (dbt-trino)"
            }
        )),
        None => o.push_str("No deployed plan: no schedule.\n\n"),
    }

    o.push_str("## 6. Quality checks\n\n");
    for m in &i.marts {
        o.push_str(&format!(
            "- **{}**: one row per {}; {}\n",
            m.mart,
            if m.grain.is_empty() {
                "(no grain)".to_string()
            } else {
                m.grain.join(", ")
            },
            if m.reconciled.is_empty() {
                "no additive measure to reconcile".to_string()
            } else {
                format!(
                    "{} equal to silver on every build",
                    m.reconciled
                        .iter()
                        .map(|x| format!("`{x}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        ));
    }
    if !i.tests.is_empty() {
        o.push_str("\n| Layer | Test | Count |\n|---|---|---|\n");
        for (layer, test, n) in &i.tests {
            o.push_str(&format!("| {layer} | {test} | {n} |\n"));
        }
    }
    o.push('\n');

    o.push_str(
        "## 7. Review of the source model\n\n| Subject | Sign-off | By | At |\n|---|---|---|---|\n",
    );
    for r in &i.review {
        o.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            r.subject,
            r.state,
            r.by.as_deref().unwrap_or("—"),
            r.at.as_deref().unwrap_or("—")
        ));
    }
    o.push('\n');

    o.push_str("## 8. Open items\n\n");
    let open = open_items(i);
    if open.is_empty() {
        o.push_str("None: every requirement and the extraction plan are approved, every review subject is signed.\n");
    }
    for x in open {
        o.push_str(&format!("- {x}\n"));
    }
    o
}

/// The dbt tests in the generated `schema.yml`-style files under `dir`, counted by name (`- unique`,
/// `- not_null`, `- <macro>:`).
pub fn count_tests(text: &str) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    // (the `tests:` key's indent, the indent of its list items once the first is seen)
    let mut open: Option<(usize, Option<usize>)> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if t == "tests:" || t == "data_tests:" {
            open = Some((indent, None));
            continue;
        }
        let Some((key, items)) = open else { continue };
        let item = t.starts_with("- ");
        // YAML puts a key's list items at the key's indent or deeper.
        let at = match items {
            None if item && indent >= key => {
                open = Some((key, Some(indent)));
                indent
            }
            Some(i) => i,
            None => {
                open = None;
                continue;
            }
        };
        if indent < at || (indent == at && !item) {
            open = None;
        } else if indent == at {
            let name = t[2..]
                .split([':', ' '])
                .next()
                .unwrap_or_default()
                .trim_start_matches("dbt_utils.");
            if !name.is_empty() {
                *counts.entry(name.to_string()).or_default() += 1;
            }
        }
    }
    counts.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input() -> SpecInput {
        SpecInput {
            project: "default/idemduck".into(),
            generated_at: "2026-11-14T20:00:00Z".into(),
            marts: vec![
                MartPart {
                    mart: "sales_by_category_month".into(),
                    prose: "Revenue of completed customer invoices.".into(),
                    requirement: "### sales_by_category_month — approved\n\nApproved by maria at 2026-11-14T19:07:05Z (digest `abc`).\n".into(),
                    state: "approved".into(),
                    grain: vec!["month".into(), "product_category".into()],
                    reconciled: vec!["revenue".into()],
                },
                MartPart {
                    mart: "loyalty".into(),
                    state: "questions".into(),
                    ..Default::default()
                },
            ],
            sources: vec![("idempiere".into(), "git".into())],
            databases: vec![("idempiere-db".into(), "postgres".into())],
            extraction: json!({
                "proposed": true,
                "summary": {"state": "approved", "approved_by": "maria", "approved_at": "2026-11-14T19:07:05Z",
                            "digest": "a8145c82", "incremental_every_run": 20, "incremental_audited": 8,
                            "incremental_other": 0, "full": 14, "refused": 0, "excluded": 860, "masked_columns": 26},
                "tables": [{"table": "adempiere.c_invoice", "mode": "incremental", "watermark": "updated",
                            "deletes": "audited weekly", "masked": 0}]
            }),
            schedule: Some("0 * * * *".into()),
            gold_engine: "duckdb".into(),
            tests: vec![("silver".into(), "not_null".into(), 120)],
            review: vec![
                Signoff { subject: "scope".into(), state: "signed".into(), by: Some("maria".into()), at: Some("2026-11-14T18:00:00Z".into()) },
                Signoff { subject: "pii".into(), state: "stale".into(), by: Some("maria".into()), at: Some("2026-11-14T18:00:00Z".into()) },
            ],
        }
    }

    #[test]
    fn every_section_comes_from_a_record_and_open_items_are_listed() {
        let md = render(&input());
        for h in [
            "## 1. Purpose",
            "## 2. Marts",
            "## 3. Sources",
            "## 4. Extraction",
            "## 5. Schedule",
            "## 6. Quality checks",
            "## 7. Review",
            "## 8. Open items",
        ] {
            assert!(md.contains(h), "{h}");
        }
        assert!(md.contains("approved by maria at 2026-11-14T19:07:05Z"));
        assert!(
            md.contains("| adempiere.c_invoice | incremental | updated | audited weekly | 0 |")
        );
        assert!(md.contains("every hour, at minute 0"));
        assert!(md.contains("gold marts built, tested and reconciled in DuckDB"));
        assert!(md.contains("`revenue` equal to silver on every build"));
        assert!(md.contains("- mart `loyalty`: requirement questions"));
        assert!(md.contains("- review `pii`: stale"));
        assert!(!md.contains("- review `scope`"));
    }

    #[test]
    fn nothing_is_called_approved_without_a_record() {
        let mut i = input();
        i.extraction = json!({"proposed": false, "summary": {"state": "not proposed"}});
        let md = render(&i);
        assert!(md.contains("No extraction plan has been proposed"));
        assert!(md.contains("- extraction plan: not proposed"));
    }

    #[test]
    fn cron_shapes_in_words() {
        assert_eq!(cron_words("0 * * * *"), "every hour, at minute 0");
        assert_eq!(cron_words("*/5 * * * *"), "every 5 minutes");
        assert_eq!(cron_words("30 6 * * *"), "every day at 06:30");
        assert_eq!(cron_words("0 6 * * 1"), "on the cron schedule `0 6 * * 1`");
    }

    #[test]
    fn tests_are_counted_by_name() {
        let yml = "models:\n  - name: a\n    columns:\n      - name: id\n        data_tests:\n          - unique\n          - not_null\n      - name: s\n        tests:\n          - accepted_values:\n              values: ['A']\n          - not_null\n  - name: b\n";
        // As the generator writes them: items at the key's indent, arguments nested below.
        let generated = "    columns:\n    - name: isactive\n      data_tests:\n      - accepted_values:\n          arguments:\n            values:\n            - Y\n            - N\n      - not_null\n    - name: id\n      data_tests:\n      - unique\n";
        assert_eq!(
            count_tests(generated),
            [
                ("accepted_values".to_string(), 1),
                ("not_null".to_string(), 1),
                ("unique".to_string(), 1)
            ]
        );
        assert_eq!(
            count_tests(yml),
            [
                ("accepted_values".to_string(), 1),
                ("not_null".to_string(), 2),
                ("unique".to_string(), 1)
            ]
        );
    }
}

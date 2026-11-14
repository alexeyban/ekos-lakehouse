//! EKOS RFC 0259 §3.2: gold marts on DuckDB. Bronze and silver stay on Trino; a mart is built by
//! dbt-duckdb, which reads silver from the same Iceberg catalog (Polaris, `ATTACH`ed as [`ALIAS`])
//! and writes the mart into one DuckDB file. Pure: the files of the dbt-duckdb project, the lowering
//! of a compiled mart into it, the Iceberg/Trino → DuckDB type map, and the cross-engine
//! reconciliation queries (each measure's total in the DuckDB file against silver in Trino, both
//! rendered in RFC 0155's canonical form through `ekos-migrate-validate`).
//!
//! Credentials are never written: the generated `profiles.yml` names the environment variables
//! that hold them (`env_var('…')`), and only endpoints, which are not secrets, have defaults.

use crate::{Compiled, MartSpec, Resolved, Term, col};
use ekos_lakehouse_bronze::plan::{Plan, gold_schema};
use ekos_lakehouse_bronze::typemap::ident;
use ekos_migrate_validate::{ColumnRule, Dialect, canon_expr_of, duckdb::DUCKDB};
use std::collections::BTreeMap;

/// The dbt-duckdb project's directory in the pipeline repository.
pub const PROJECT_DIR: &str = "gold-duckdb";

/// The name the Iceberg catalog is attached under in DuckDB.
pub const ALIAS: &str = "lh";

/// The DuckDB secret the `ATTACH` authenticates with (created from `profiles.yml`).
pub const ICEBERG_SECRET: &str = "ekos_lh_iceberg";

/// The environment variable that names the file dbt-duckdb builds into (`marts.next.duckdb`):
/// the publish step sets it, so a build never touches the published file.
pub const NEXT_ENV: &str = "EKOS_LH_MARTS_NEXT";

/// DuckDB's version every consumer of the marts file is pinned to (the Dagster image that writes
/// it, the Superset image and EKOS's probe that read it): a file is read by the version that wrote
/// it or a later one.
pub const DUCKDB_VERSION: &str = "1.5.6";

/// Where dbt-duckdb reads silver and how it authenticates. Endpoints have defaults in the
/// generated files and can be overridden from the environment at run time (the orchestrator's
/// container reaches the services under other names); secrets are only ever named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckDbGold {
    /// Polaris' Iceberg REST endpoint, `http://host:port/api/catalog`.
    pub catalog_url: String,
    /// The Iceberg catalog (warehouse) to attach.
    pub warehouse: String,
    /// The S3 endpoint as DuckDB takes it: `host:port`, no scheme.
    pub s3_endpoint: String,
    pub s3_use_ssl: bool,
    pub s3_region: String,
    /// The environment variables holding the S3 key pair and the catalog's OAuth2 client.
    pub s3_key_id_env: String,
    pub s3_secret_env: String,
    pub client_id_env: String,
    pub client_secret_env: String,
}

impl Default for DuckDbGold {
    fn default() -> Self {
        Self {
            catalog_url: "http://127.0.0.1:58181/api/catalog".into(),
            warehouse: "lakehouse".into(),
            s3_endpoint: "127.0.0.1:58333".into(),
            s3_use_ssl: false,
            s3_region: "us-east-1".into(),
            s3_key_id_env: "AWS_ACCESS_KEY_ID".into(),
            s3_secret_env: "AWS_SECRET_ACCESS_KEY".into(),
            client_id_env: "EKOS_LH_POLARIS_CLIENT_ID".into(),
            client_secret_env: "EKOS_LH_POLARIS_CLIENT_SECRET".into(),
        }
    }
}

/// `http://host:port` → (`host:port`, uses TLS).
pub fn s3_endpoint_of(url: &str) -> (String, bool) {
    let tls = url.starts_with("https://");
    let rest = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    (rest.to_string(), tls)
}

// ─────────────────────────────── types ───────────────────────────────

/// An Iceberg column type as Trino writes it (the plan's `iceberg_type`) → the type DuckDB reads
/// it as. `None` for a type the map does not know: never guessed.
pub fn duckdb_type(iceberg: &str) -> Option<String> {
    let t = iceberg.trim().to_ascii_lowercase();
    let simple = match t.as_str() {
        "boolean" => "BOOLEAN",
        "tinyint" => "TINYINT",
        "smallint" => "SMALLINT",
        "integer" | "int" => "INTEGER",
        "bigint" => "BIGINT",
        "real" => "FLOAT",
        "double" => "DOUBLE",
        "date" => "DATE",
        "varchar" => "VARCHAR",
        "varbinary" => "BLOB",
        "uuid" => "UUID",
        "time(6)" | "time" => "TIME",
        "timestamp(6)" | "timestamp" => "TIMESTAMP",
        "timestamp(6) with time zone" | "timestamp with time zone" => "TIMESTAMPTZ",
        _ => "",
    };
    if !simple.is_empty() {
        return Some(simple.into());
    }
    if let Some(args) = t.strip_prefix("decimal(").and_then(|r| r.strip_suffix(')')) {
        let (p, s) = args.split_once(',').unwrap_or((args, "0"));
        let (p, s): (u32, u32) = (p.trim().parse().ok()?, s.trim().parse().ok()?);
        // DuckDB's widest decimal is 38 digits, as Iceberg's.
        return (p <= 38 && s <= p).then(|| format!("DECIMAL({p},{s})"));
    }
    if t.starts_with("varchar(") {
        return Some("VARCHAR".into());
    }
    None
}

/// The canonical-form rule for a value of this Iceberg type, when the type has an exact one.
fn rule_of(iceberg: &str) -> Option<ColumnRule> {
    let t = iceberg.trim().to_ascii_lowercase();
    match t.as_str() {
        "tinyint" | "smallint" | "integer" | "int" | "bigint" => Some(ColumnRule::Int),
        _ => t
            .strip_prefix("decimal(")
            .and_then(|r| r.strip_suffix(')'))
            .and_then(|a| a.split_once(','))
            .and_then(|(_, s)| s.trim().parse().ok())
            .map(ColumnRule::Decimal),
    }
}

// ─────────────────────────────── lowering ───────────────────────────────

/// A silver model's DuckDB source: (source name = its Iceberg namespace, table as the model name).
fn source_of(plan: &Plan, model: &str) -> (String, String) {
    if let Some(t) = plan
        .tables
        .iter()
        .find(|t| ekos_lakehouse_silver::model_name(t) == model)
    {
        return (ekos_lakehouse_silver::silver_schema(t), model.to_string());
    }
    // A model the plan does not hold (a Pentaho stage, a stale ref): named as `engine_sql` does.
    let (schema, _) = model.split_once("__").unwrap_or(("public", model));
    let tag = ekos_lakehouse_bronze::plan::layer_tag(&plan.settings.namespace_prefix);
    (format!("silver_{tag}{schema}"), model.to_string())
}

/// A compiled mart's dbt SQL (or one of its tests) for the dbt-duckdb project: each silver
/// `{{ ref('<model>') }}` becomes `{{ source('<silver schema>', '<model>') }}` — silver is not
/// built there, it is read from the attached catalog — and a gold ref stays a ref. The source
/// keeps the model's name as its table name (its `identifier` is the Iceberg table), so the SQL
/// with refs and sources replaced by names reads the same for both engines.
pub fn lower(dbt_sql: &str, plan: &Plan) -> String {
    let mut out = String::new();
    let mut rest = dbt_sql;
    while let Some(i) = rest.find("{{ ref('") {
        out.push_str(&rest[..i]);
        let after = &rest[i + "{{ ref('".len()..];
        let Some(j) = after.find("') }}") else {
            out.push_str(&rest[i..]);
            return out;
        };
        let model = &after[..j];
        if model.starts_with("gold__") {
            out.push_str(&format!("{{{{ ref('{model}') }}}}"));
        } else {
            let (source, table) = source_of(plan, model);
            out.push_str(&format!("{{{{ source('{source}', '{table}') }}}}"));
        }
        rest = &after[j + "') }}".len()..];
    }
    out.push_str(rest);
    out
}

// ─────────────────────────────── the dbt-duckdb project ───────────────────────────────

fn yaml_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn dbt_project() -> String {
    format!(
        "# Generated by ekos-lakehouse (EKOS RFC 0259). Do not edit: regenerate.\n\
         # Gold marts on DuckDB: silver is read from the Iceberg catalog attached as `{ALIAS}`,\n\
         # marts are written into the file `{NEXT_ENV}` names; `lakehouse gold publish` builds,\n\
         # tests and reconciles it before it replaces the published marts file.\n\
         name: ekos_lakehouse_gold\n\
         version: \"1.0\"\n\
         config-version: 2\n\
         profile: ekos_lakehouse_gold\n\
         model-paths: [\"models\"]\n\
         test-paths: [\"tests\"]\n\
         macro-paths: [\"macros\"]\n\
         on-run-start:\n  - \"{{{{ ekos_attach_silver() }}}}\"\n"
    )
}

fn attach_macro(s: &DuckDbGold) -> String {
    format!(
        "{{# Generated by ekos-lakehouse (EKOS RFC 0259). Do not edit: regenerate.\n   \
         The Iceberg catalog silver lives in, read-only for marts: attached once per run, before\n   \
         any model. The secret `{ICEBERG_SECRET}` comes from profiles.yml; the endpoint can be\n   \
         overridden from the environment. #}}\n\
         {{% macro ekos_attach_silver() %}}\n\
         ATTACH IF NOT EXISTS '{{{{ env_var('EKOS_LH_ICEBERG_WAREHOUSE', {warehouse}) }}}}' AS {ALIAS} (\n    \
         TYPE iceberg,\n    \
         ENDPOINT '{{{{ env_var('EKOS_LH_ICEBERG_URL', {url}) }}}}',\n    \
         SECRET {ICEBERG_SECRET},\n    \
         ACCESS_DELEGATION_MODE 'none'\n)\n\
         {{% endmacro %}}\n",
        warehouse = yaml_quote(&s.warehouse),
        url = yaml_quote(&s.catalog_url),
    )
}

fn profiles(s: &DuckDbGold) -> String {
    let env = |name: &str| format!("\"{{{{ env_var('{name}') }}}}\"");
    let oauth = format!(
        "\"{{{{ env_var('EKOS_LH_ICEBERG_URL', {}) }}}}/v1/oauth/tokens\"",
        yaml_quote(&s.catalog_url)
    );
    let env_or = |name: &str, default: &str| {
        format!("\"{{{{ env_var('{name}', {}) }}}}\"", yaml_quote(default))
    };
    format!(
        "# Generated by ekos-lakehouse (EKOS RFC 0259). Do not edit: regenerate.\n\
         # Secrets are named by environment variable only; nothing secret is written here.\n\
         ekos_lakehouse_gold:\n  \
         target: marts\n  \
         outputs:\n    \
         marts:\n      \
         type: duckdb\n      \
         path: {path}\n      \
         threads: 4\n      \
         extensions:\n        - httpfs\n        - iceberg\n      \
         settings:\n        \
         extension_directory: \"{{{{ env_var('EKOS_LH_DUCKDB_EXTENSIONS', '~/.duckdb/extensions') }}}}\"\n      \
         secrets:\n        \
         - type: s3\n          \
         name: ekos_lh_s3\n          \
         key_id: {key}\n          \
         secret: {secret}\n          \
         endpoint: {endpoint}\n          \
         url_style: path\n          \
         use_ssl: {ssl}\n          \
         region: {region}\n        \
         - type: iceberg\n          \
         name: {ICEBERG_SECRET}\n          \
         client_id: {client}\n          \
         client_secret: {client_secret}\n          \
         oauth2_server_uri: {oauth}\n          \
         oauth2_scope: PRINCIPAL_ROLE:ALL\n",
        path = env_or(NEXT_ENV, "marts.next.duckdb"),
        key = env(&s.s3_key_id_env),
        secret = env(&s.s3_secret_env),
        endpoint = env_or("EKOS_LH_S3_ENDPOINT", &s.s3_endpoint),
        ssl = s.s3_use_ssl,
        region = yaml_quote(&s.s3_region),
        client = env(&s.client_id_env),
        client_secret = env(&s.client_secret_env),
        oauth = oauth,
    )
}

/// Silver as dbt sources: one source per silver schema, read from the attached catalog, each table
/// named as its silver model with the Iceberg table as its identifier.
fn sources(plan: &Plan) -> String {
    use serde_yaml::{Mapping, Value};
    let mut by_schema: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for t in &plan.tables {
        let mut table = Mapping::new();
        table.insert("name".into(), ekos_lakehouse_silver::model_name(t).into());
        table.insert(
            "identifier".into(),
            ekos_lakehouse_bronze::iceberg_name(&t.table).into(),
        );
        by_schema
            .entry(ekos_lakehouse_silver::silver_schema(t))
            .or_default()
            .push(Value::Mapping(table));
    }
    let list: Vec<Value> = by_schema
        .into_iter()
        .map(|(schema, tables)| {
            let mut m = Mapping::new();
            m.insert("name".into(), schema.clone().into());
            m.insert("database".into(), ALIAS.into());
            m.insert("schema".into(), schema.into());
            m.insert("tables".into(), Value::Sequence(tables));
            Value::Mapping(m)
        })
        .collect();
    let mut root = Mapping::new();
    root.insert("version".into(), 2.into());
    root.insert("sources".into(), Value::Sequence(list));
    format!(
        "# Generated by ekos-lakehouse (EKOS RFC 0259). Silver, built by the dbt-trino project,\n\
         # read here from the Iceberg catalog.\n{}",
        serde_yaml::to_string(&Value::Mapping(root)).expect("YAML serializes")
    )
}

const GITIGNORE: &str = "# Generated by ekos-lakehouse (EKOS RFC 0259).\ntarget/\nlogs/\n\
dbt_packages/\n.user.yml\n*.duckdb\n*.duckdb.wal\n";

/// The dbt-duckdb project's own files (not the marts: those arrive by proposal), by path in the
/// pipeline repository. Written by `lakehouse generate` when `[extensions.lakehouse.gold]` says
/// `engine = "duckdb"`.
pub fn project_files(plan: &Plan, s: &DuckDbGold) -> BTreeMap<String, String> {
    let d = PROJECT_DIR;
    BTreeMap::from([
        (format!("{d}/dbt_project.yml"), dbt_project()),
        (format!("{d}/profiles.yml"), profiles(s)),
        (format!("{d}/.gitignore"), GITIGNORE.to_string()),
        (format!("{d}/models/_silver_sources.yml"), sources(plan)),
        (
            format!("{d}/macros/generate_schema_name.sql"),
            ekos_lakehouse_silver::SCHEMA_MACRO.to_string(),
        ),
        (
            format!("{d}/macros/ekos_unique_combination.sql"),
            ekos_lakehouse_silver::UNIQUE_COMBINATION.to_string(),
        ),
        (
            format!("{d}/macros/ekos_attach_silver.sql"),
            attach_macro(s),
        ),
    ])
}

// ─────────────────────────────── reconciliation across engines ───────────────────────────────

/// One additive measure, checked across engines: its total over the mart in the DuckDB file, and
/// the same aggregate over silver in Trino under the mart's joins and filters — each rendered in
/// the canonical form of its engine, so equal totals are equal text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossCheck {
    pub measure: String,
    /// For DuckDB, over the marts file.
    pub mart_sql: String,
    /// For Trino, over silver.
    pub silver_sql: String,
    /// `canonical` when both sides render through RFC 0155's rule for the type; `numeric` for a
    /// floating-point measure, compared as numbers (RFC 0246's `values_agree`).
    pub compare: &'static str,
}

/// Every additive (`sum`/`count`, not distinct) measure of a DuckDB mart as a [`CrossCheck`].
/// `catalog` is the Trino catalog silver is read through.
pub fn cross_checks(
    spec: &MartSpec,
    resolved: &[Resolved],
    c: &Compiled,
    plan: &Plan,
    catalog: &str,
) -> Vec<CrossCheck> {
    let engine = crate::engine_sql(c, catalog);
    let from_where = engine
        .split_once("\nfrom ")
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    let from_where = from_where.split("\ngroup by").next().unwrap_or(from_where);
    let mart = format!(
        "{}.{}",
        ident(&gold_schema(&plan.settings.namespace_prefix)),
        ident(&spec.mart)
    );
    let type_of = |table: &str, column: &str| -> Option<String> {
        plan.tables
            .iter()
            .find(|t| t.table == table)?
            .columns
            .iter()
            .find(|m| m.column == column)
            .map(|m| m.iceberg_type.clone())
    };
    let mut out = vec![];
    for r in resolved.iter().filter(|r| r.section == "measures") {
        let Ok(Term::Aggregate {
            func,
            distinct: false,
            arg,
            ..
        }) = &r.resolution
        else {
            continue;
        };
        if func != "sum" && func != "count" {
            continue;
        }
        let (silver, rule) = match arg {
            Some(a) if func == "sum" => (
                format!("sum({})", col(a)),
                type_of(&a.table, &a.column).and_then(|t| rule_of(&t)),
            ),
            Some(a) => (format!("count({})", col(a)), Some(ColumnRule::Int)),
            None => ("count(*)".to_string(), Some(ColumnRule::Int)),
        };
        let total = format!("sum({})", ident(&r.term));
        let (mart_expr, silver_expr, compare) = match rule {
            Some(rule) => (
                canon_expr_of(Dialect::Registered(&DUCKDB), &total, rule),
                canon_expr_of(Dialect::Trino, &silver, rule),
                "canonical",
            ),
            None => (
                format!("CAST({total} AS VARCHAR)"),
                format!("CAST({silver} AS varchar)"),
                "numeric",
            ),
        };
        out.push(CrossCheck {
            measure: r.term.clone(),
            mart_sql: format!("SELECT {mart_expr} AS mart_total FROM {mart}"),
            silver_sql: format!("select {silver_expr} as silver_total\nfrom {from_where}"),
            compare,
        });
    }
    out
}

/// Whether a cross-engine check's two answers agree: canonical forms are compared as text,
/// floating-point totals as numbers.
pub fn agrees(check: &CrossCheck, mart_total: &str, silver_total: &str) -> bool {
    match check.compare {
        "canonical" => mart_total == silver_total,
        _ => ekos_migrate_validate::metrics::values_agree(mart_total, silver_total),
    }
}

#[cfg(test)]
mod tests;

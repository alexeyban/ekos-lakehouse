//! `lakehouse install` (RFC 0199 §2): the stack, a first project and the web console, in one
//! command, under the user's EKOS home. Nothing outside the home changes except Docker's own
//! containers and volumes, and the console's login service when asked for.
//!
//! The stack's files are compiled into the binary, so a binary and the stack it drives never
//! disagree. A project is a project in the EKOS home (RFC 0178): its `ekos.toml` and ledger are
//! in the home, its pipeline repository in `<home>/lakehouse/projects/<name>`, and the console
//! lists it on start.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Every file of the stack, by its path under `<home>/lakehouse/stack`.
const STACK: &[(&str, &str, bool)] = &[
    (
        "docker-compose.lakehouse.yml",
        include_str!("../../../deploy/docker-compose.lakehouse.yml"),
        false,
    ),
    (
        "docker-compose.home.yml",
        include_str!("../../../deploy/docker-compose.home.yml"),
        false,
    ),
    (
        "trino/catalog/iceberg.properties",
        include_str!("../../../deploy/trino/catalog/iceberg.properties"),
        false,
    ),
    (
        "polaris/create-catalog.sh",
        include_str!("../../../deploy/polaris/create-catalog.sh"),
        true,
    ),
    (
        "seaweedfs/s3.json",
        include_str!("../../../deploy/seaweedfs/s3.json"),
        false,
    ),
    (
        "dagster/Dockerfile",
        include_str!("../../../deploy/dagster/Dockerfile"),
        false,
    ),
    (
        "dagster/placeholder.py",
        include_str!("../../../deploy/dagster/placeholder.py"),
        false,
    ),
    (
        "otel/collector.yaml",
        include_str!("../../../deploy/otel/collector.yaml"),
        false,
    ),
];

/// Ports the stack binds on loopback.
const PORTS: &[(u16, &str)] = &[
    (55433, "sample PostgreSQL"),
    (58333, "storage (S3)"),
    (58181, "Polaris"),
    (58182, "Polaris health"),
    (58080, "Trino"),
    (53000, "Dagster"),
];

/// The sandbox source's role and password (local only, like every credential of the stack).
const SAMPLE_USER: &str = "ekos";
const SAMPLE_PASSWORD: &str = "ekos-local-only";
const SAMPLE_DB: &str = "shop";

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub home: PathBuf,
    /// `host:port/database` of the user's PostgreSQL; `None` installs the sample `shop`.
    pub source: Option<String>,
    pub source_user: String,
    /// The variable holding the source password at install time.
    pub source_password_env: String,
    pub project: Option<String>,
    pub schemas: Vec<String>,
    pub console: bool,
    pub open: bool,
    pub at_login: bool,
    pub first_run: bool,
    pub console_port: u16,
}

/// A parsed `--source host:port/database`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub host: String,
    pub port: u16,
    pub database: String,
}

impl Source {
    pub fn parse(s: &str) -> Result<Self> {
        let (hp, database) = s
            .split_once('/')
            .context("--source is host:port/database")?;
        let (host, port) = match hp.rsplit_once(':') {
            Some((h, p)) => (h, p.parse().context("--source: the port is a number")?),
            None => (hp, 5432),
        };
        if host.is_empty() || database.is_empty() || host.contains('@') {
            bail!("--source is host:port/database (no user or password in it)");
        }
        Ok(Self {
            host: host.into(),
            port,
            database: database.into(),
        })
    }

    /// How Trino, inside Docker, reaches it: this machine is `host.docker.internal` there.
    pub fn for_trino(&self) -> String {
        let host = match self.host.as_str() {
            "localhost" | "127.0.0.1" | "::1" => "host.docker.internal",
            h => h,
        };
        format!("{host}:{}", self.port)
    }
}

/// A project or catalog name from a database name: lowercase letters, digits and `_`.
pub fn name_for(db: &str) -> String {
    let mut n: String = db
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if !n.starts_with(|c: char| c.is_ascii_lowercase()) {
        n.insert_str(0, "db_");
    }
    n
}

/// The Trino catalog file for a source; its password comes from `var` in Trino's environment.
pub fn catalog_properties(src: &Source, user: &str, var: &str) -> String {
    format!(
        "# Written by `ekos-lakehouse lakehouse install` (RFC 0199).\n\
         connector.name=postgresql\n\
         connection-url=jdbc:postgresql://{}/{}\n\
         connection-user={user}\n\
         connection-password=${{ENV:{var}}}\n\
         # Without these two, Trino silently leaves out columns it does not map (RFC 0192 §1).\n\
         postgresql.array-mapping=AS_ARRAY\n\
         unsupported-type-handling=CONVERT_TO_VARCHAR\n",
        src.for_trino(),
        src.database
    )
}

/// The project's `[extensions.lakehouse]` and `[observe]` sections.
pub fn project_settings(
    src: &Source,
    catalog: &str,
    user: &str,
    password_file: &str,
    schemas: &[String],
) -> String {
    let mut t = format!(
        "\n# EKOS Lakehouse (RFC 0199), written by `ekos-lakehouse lakehouse install`.\n\
         [extensions.lakehouse]\n\
         trino-url = \"http://127.0.0.1:58080\"\n\
         polaris-health-url = \"http://127.0.0.1:58182/q/health\"\n\
         s3-url = \"http://127.0.0.1:58333\"\n\
         dagster-url = \"http://127.0.0.1:53000\"\n\
         source = \"{}:{}\"\n\
         source-database = \"{}\"\n\
         source-user = \"{user}\"\n\
         source-password-file = \"{password_file}\"\n\
         source-catalog = \"{catalog}\"\n\
         bronze-schedule = \"0 * * * *\"\n",
        src.host, src.port, src.database
    );
    if !schemas.is_empty() {
        let list: Vec<String> = schemas.iter().map(|s| format!("\"{s}\"")).collect();
        t.push_str(&format!("schemas = [{}]\n", list.join(", ")));
    }
    t
}

fn say(msg: &str) {
    println!("==> {msg}");
}

fn run(cmd: &mut Command, what: &str) -> Result<String> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("{what}: cannot start {:?}", cmd.get_program()))?;
    if !out.status.success() {
        bail!(
            "{what} failed ({}):\n{}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::File::create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(text.as_bytes())?;
    Ok(())
}

/// Where the installer keeps the stack and the pipeline repositories.
pub fn stack_dir(home: &Path) -> PathBuf {
    home.join("lakehouse").join("stack")
}

pub fn projects_dir(home: &Path) -> PathBuf {
    home.join("lakehouse").join("projects")
}

/// Write the stack's files; `secrets.env` and catalogs the installer added are kept.
pub fn write_stack(dir: &Path) -> Result<()> {
    for (rel, text, exec) in STACK {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent"))?;
        std::fs::write(&p, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if *exec { 0o755 } else { 0o644 };
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))?;
        }
    }
    let catalogs = dir.join("trino/catalog");
    // The sample source's catalog: `postgresql` reads the sandbox's `source` database.
    std::fs::write(
        catalogs.join(format!("{SAMPLE_DB}.properties")),
        format!(
            "# The sample source (RFC 0199). Written by `ekos-lakehouse lakehouse install`.\n\
             connector.name=postgresql\n\
             connection-url=jdbc:postgresql://lh-postgres:5432/{SAMPLE_DB}\n\
             connection-user={SAMPLE_USER}\n\
             connection-password=${{ENV:PG_PASSWORD}}\n\
             postgresql.array-mapping=AS_ARRAY\n\
             unsupported-type-handling=CONVERT_TO_VARCHAR\n"
        ),
    )?;
    Ok(())
}

fn compose(dir: &Path, home_mode: bool) -> Command {
    let mut c = Command::new("docker");
    c.arg("compose")
        .arg("--project-directory")
        .arg(dir)
        .arg("-f")
        .arg(dir.join("docker-compose.lakehouse.yml"));
    if home_mode {
        c.arg("-f").arg(dir.join("docker-compose.home.yml"));
    }
    c
}

/// Docker and Compose v2 are there and answer.
pub fn check_docker() -> Result<()> {
    run(
        Command::new("docker").args(["version", "--format", "{{.Server.Version}}"]),
        "Docker (is it installed and running, and may this user use it?)",
    )?;
    run(
        Command::new("docker").args(["compose", "version"]),
        "Docker Compose v2",
    )?;
    Ok(())
}

/// The stack's ports that something other than this stack holds.
pub fn busy_ports() -> Vec<(u16, &'static str)> {
    let ours = Command::new("docker")
        .args([
            "ps",
            "--filter",
            "label=com.docker.compose.project=ekos-lakehouse",
            "--format",
            "{{.Names}} {{.Ports}}",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    PORTS
        .iter()
        .filter(|(p, _)| {
            // Dagster runs on the host network, so Docker lists no port for it.
            let ours_dagster = *p == 53000 && ours.contains("lh-dagster");
            std::net::TcpListener::bind(("127.0.0.1", *p)).is_err()
                && !ours.contains(&format!("127.0.0.1:{p}->"))
                && !ours_dagster
        })
        .copied()
        .collect()
}

fn wait_for(what: &str, url: &str, ok: impl Fn(&str) -> bool, limit: Duration) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let started = Instant::now();
    while started.elapsed() < limit {
        if let Ok(r) = client.get(url).send()
            && let Ok(body) = r.text()
            && ok(&body)
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(3));
    }
    bail!(
        "{what} did not become ready at {url} within {}s",
        limit.as_secs()
    )
}

/// Start (or update) the stack and wait until Trino can be used.
pub fn stack_up(dir: &Path) -> Result<()> {
    say("starting the stack (the first time downloads about 3 GB of images)");
    // No `--wait`: the bucket and catalog setup containers exit when done, which `--wait`
    // reports as a failure; readiness is checked below instead.
    run(compose(dir, false).args(["up", "-d"]), "docker compose up")?;
    wait_for(
        "Trino",
        "http://127.0.0.1:58080/v1/info",
        |b| b.contains("\"starting\":false"),
        Duration::from_secs(300),
    )?;
    wait_for(
        "Polaris",
        "http://127.0.0.1:58182/q/health",
        |b| b.contains("UP"),
        Duration::from_secs(120),
    )?;
    Ok(())
}

fn restart_trino(dir: &Path) -> Result<()> {
    run(
        compose(dir, false).args(["up", "-d", "--force-recreate", "--no-deps", "lh-trino"]),
        "restarting Trino",
    )?;
    wait_for(
        "Trino",
        "http://127.0.0.1:58080/v1/info",
        |b| b.contains("\"starting\":false"),
        Duration::from_secs(300),
    )
}

/// The sample `shop` database in the sandbox PostgreSQL: customers and orders, with the
/// trigger that lets `orders` load incrementally (the quickstart's example).
fn create_sample(dir: &Path) -> Result<()> {
    let sql = "SELECT 'CREATE DATABASE shop' WHERE NOT EXISTS \
               (SELECT FROM pg_database WHERE datname = 'shop')\\gexec\n";
    psql(dir, "source", sql)?;
    psql(
        dir,
        SAMPLE_DB,
        "CREATE TABLE IF NOT EXISTS customers (id int PRIMARY KEY, name text NOT NULL, email text, country text);\n\
         CREATE TABLE IF NOT EXISTS orders (id int PRIMARY KEY, customer_id int NOT NULL REFERENCES customers (id),\n\
           amount numeric(12,2) NOT NULL, status text NOT NULL, updated_at timestamptz NOT NULL DEFAULT now());\n\
         CREATE OR REPLACE FUNCTION touch() RETURNS trigger LANGUAGE plpgsql AS\n\
           $$ BEGIN NEW.updated_at := now(); RETURN NEW; END $$;\n\
         DROP TRIGGER IF EXISTS touch ON orders;\n\
         CREATE TRIGGER touch BEFORE UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION touch();\n\
         INSERT INTO customers SELECT i, 'Customer ' || i, 'c' || i || '@example.com', 'DE'\n\
           FROM generate_series(1, 200) i ON CONFLICT DO NOTHING;\n\
         INSERT INTO orders SELECT i, 1 + i % 200, (i % 97) * 1.25, 'paid', now()\n\
           FROM generate_series(1, 5000) i ON CONFLICT DO NOTHING;\n\
         ANALYZE;\n",
    )?;
    Ok(())
}

fn psql(dir: &Path, db: &str, sql: &str) -> Result<()> {
    let mut c = compose(dir, false);
    c.args([
        "exec",
        "-T",
        "lh-postgres",
        "psql",
        "-v",
        "ON_ERROR_STOP=1",
        "-q",
        "-U",
        SAMPLE_USER,
        "-d",
        db,
    ]);
    let mut child = c
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("docker compose exec psql")?;
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(sql.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("psql on {db}: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Add `KEY=value` to `secrets.env` (owner-readable), replacing an earlier value of the key.
pub fn set_secret(dir: &Path, key: &str, value: &str) -> Result<()> {
    if value.contains('\n') {
        bail!("a password with a line break cannot be passed to Trino");
    }
    let path = dir.join("secrets.env");
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with(&format!("{key}=")))
        .map(str::to_string)
        .collect();
    lines.push(format!("{key}={value}"));
    write_private(&path, &(lines.join("\n") + "\n"))
}

/// The binary to run follow-up commands with: this one.
fn me() -> Result<PathBuf> {
    std::env::current_exe().context("cannot locate this binary")
}

fn ekos(home: &Path, args: &[&str], what: &str) -> Result<String> {
    run(Command::new(me()?).env("EKOS_HOME", home).args(args), what)
}

/// Create the project (if new): pipeline directory, password file, home project, settings,
/// `init`, `plan`, `generate`.
fn create_project(
    o: &Options,
    name: &str,
    src: &Source,
    catalog: &str,
    password: &str,
) -> Result<PathBuf> {
    let dir = projects_dir(&o.home).join(name);
    std::fs::create_dir_all(&dir)?;
    // The pipeline reads its project's name from here (one Dagster for every project, RFC 0199).
    std::fs::create_dir_all(dir.join(".lakehouse"))?;
    std::fs::write(
        dir.join(".lakehouse").join("project.json"),
        serde_json::to_string_pretty(&serde_json::json!({"name": name, "home": o.home}))? + "\n",
    )?;
    write_private(
        &dir.join(".lakehouse").join("source-password"),
        &format!("{password}\n"),
    )?;
    let config = o
        .home
        .join("data")
        .join("default")
        .join(name)
        .join("ekos.toml");
    if !config.is_file() {
        say(&format!("creating project {name}"));
        ekos(
            &o.home,
            &[
                "project",
                "add",
                name,
                "--source",
                dir.to_str().context("utf-8 path")?,
            ],
            "ekos project add",
        )?;
        let mut text = std::fs::read_to_string(&config)?;
        // The pipeline repository is the project's only input; the lock file and the generated
        // code are what `build` observes.
        text.push_str(&project_settings(
            src,
            catalog,
            if o.source.is_some() {
                &o.source_user
            } else {
                SAMPLE_USER
            },
            ".lakehouse/source-password",
            &o.schemas,
        ));
        if o.source.is_none() {
            // The sample's `orders` has a trigger that moves `updated_at` on every update: the
            // proof an incremental load needs (RFC 0192 §2), as in the quickstart.
            text.push_str(
                "\n[extensions.lakehouse.watermarks]\n\"public.orders\" = \"updated_at\"\n",
            );
        }
        std::fs::write(&config, text)?;
    } else {
        say(&format!("project {name} exists; keeping its settings"));
    }
    let p = ["--project", name];
    say("planning: reading the source's catalog");
    let plan = ekos(
        &o.home,
        &[&p[..], &["lakehouse", "plan"]].concat(),
        "lakehouse plan",
    )?;
    print!("{plan}");
    say("generating the pipeline repository");
    ekos(
        &o.home,
        &[&p[..], &["lakehouse", "generate"]].concat(),
        "lakehouse generate",
    )?;
    Ok(dir)
}

fn orchestrator(o: &Options) -> Command {
    let mut c = compose(&stack_dir(&o.home), true);
    // Every project's pipeline is under the projects directory, and the home is mounted too.
    c.env("EKOS_HOME", &o.home)
        .env("EKOS_LH_PROJECT", projects_dir(&o.home))
        .env("EKOS_LAKEHOUSE_BIN", me().unwrap_or_default());
    #[cfg(unix)]
    {
        // The container writes the project's ledger and Dagster's state as this user.
        let (uid, gid) = ids();
        c.env("EKOS_LH_UID", uid).env("EKOS_LH_GID", gid);
    }
    c
}

#[cfg(unix)]
fn ids() -> (String, String) {
    let id = |f: &str| {
        Command::new("id")
            .arg(f)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "1000".into())
    };
    (id("-u"), id("-g"))
}

/// Dagster's workspace: one code location per project with a generated pipeline.
pub fn workspace_yaml(home: &Path) -> Result<String> {
    let mut out = String::from(
        "# Written by `ekos-lakehouse lakehouse install` (RFC 0199): one code location per project.\nload_from:\n",
    );
    let mut any = false;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(projects_dir(home))
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    dirs.sort();
    for d in dirs {
        let defs = d.join("pipeline/dagster/definitions.py");
        let Some(name) = d.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !defs.is_file() || !d.join(".lakehouse/project.json").is_file() {
            continue;
        }
        any = true;
        out.push_str(&format!(
            "  - python_file:\n      relative_path: {}\n      working_directory: {}\n      location_name: {name}\n",
            defs.display(),
            d.join("pipeline/dagster").display()
        ));
    }
    if !any {
        bail!(
            "no project with a generated pipeline under {}",
            projects_dir(home).display()
        );
    }
    Ok(out)
}

/// Bring every other installed project's pipeline up to this binary's templates, from its
/// deployed plan (no source connection), and give it its `project.json`. A project that fails is
/// reported and left out of Dagster's workspace, never fatal to this install.
fn refresh_projects(o: &Options, current: &str) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(projects_dir(&o.home)) else {
        return Ok(());
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n != current)
        .collect();
    names.sort();
    for name in names {
        let dir = projects_dir(&o.home).join(&name);
        let config = o
            .home
            .join("data")
            .join("default")
            .join(&name)
            .join("ekos.toml");
        let lock = dir.join("pipeline").join("lakehouse.lock.json");
        if !config.is_file() || !lock.is_file() {
            continue;
        }
        std::fs::create_dir_all(dir.join(".lakehouse"))?;
        std::fs::write(
            dir.join(".lakehouse").join("project.json"),
            serde_json::to_string_pretty(&serde_json::json!({"name": name, "home": o.home}))?
                + "\n",
        )?;
        let out = ekos(
            &o.home,
            &["--project", &name, "lakehouse", "generate", "--from-lock"],
            "lakehouse generate --from-lock",
        );
        match out {
            Ok(_) => say(&format!("project {name}: pipeline refreshed")),
            Err(e) => {
                println!("warning: project {name} left out of Dagster: {e}");
                let _ = std::fs::remove_file(dir.join(".lakehouse").join("project.json"));
            }
        }
    }
    Ok(())
}

/// Build the orchestrator image, run the new project's pipeline once, then (re)start Dagster
/// with every project of the home on its schedule.
fn start_orchestrator(o: &Options, project_dir: &Path) -> Result<()> {
    let dagster_home = o.home.join("lakehouse").join("dagster");
    // Dagster refuses a DAGSTER_HOME that does not exist, and Docker would create it as root.
    std::fs::create_dir_all(&dagster_home)?;
    std::fs::write(
        dagster_home.join("workspace.yaml"),
        workspace_yaml(&o.home)?,
    )?;
    say("building the orchestrator image (Dagster, dbt)");
    run(
        orchestrator(o).args(["--profile", "orchestrator", "build", "lh-dagster"]),
        "building the Dagster image",
    )?;
    if o.first_run {
        say("running the pipeline once: bronze, reconciliation, silver (a few minutes)");
        let defs = project_dir.join("pipeline/dagster/definitions.py");
        let defs = defs.to_str().context("utf-8 path")?;
        run(
            orchestrator(o).args([
                "--profile",
                "orchestrator",
                "run",
                "--rm",
                "lh-dagster",
                "dagster",
                "job",
                "execute",
                "-f",
                defs,
                "-j",
                "bronze",
            ]),
            "the first pipeline run",
        )?;
    }
    say("starting Dagster with every project on its schedule: http://127.0.0.1:53000");
    run(
        orchestrator(o).args([
            "--profile",
            "orchestrator",
            "up",
            "-d",
            "--force-recreate",
            "lh-dagster",
        ]),
        "starting Dagster",
    )?;
    Ok(())
}

/// The web console in local mode, through `setup` (a full package only).
fn start_console(o: &Options) -> Result<bool> {
    let exe = me()?;
    let package = exe
        .parent()
        .and_then(Path::parent)
        .context("package layout")?;
    if !package
        .join("console")
        .join("app")
        .join("main.py")
        .is_file()
    {
        println!(
            "\nThe web console is not part of this build (no console/ beside bin/). Install the \
             release package (see docs/install.md), or start a console from an EKOS checkout:\n  \
             EKOS_BIN={} … uvicorn app.main:create_app --factory",
            exe.display()
        );
        return Ok(false);
    }
    say("setting up the web console (local mode)");
    let answers = format!(
        "home = \"{}\"\n\
         add_to_path = true\n\
         [console]\nmode = \"local\"\nhost = \"127.0.0.1\"\nport = {}\n\
         [start]\nat_login = {}\nnow = true\nopen = {}\n",
        o.home.display(),
        o.console_port,
        o.at_login,
        o.open
    );
    let file = o.home.join("lakehouse").join("setup-answers.toml");
    std::fs::write(&file, answers)?;
    let out = ekos(
        &o.home,
        &["setup", "--answers", file.to_str().context("utf-8 path")?],
        "ekos setup",
    )?;
    print!("{out}");
    Ok(true)
}

/// The whole install.
pub fn install(o: &Options) -> Result<()> {
    check_docker()?;
    let busy = busy_ports();
    if !busy.is_empty() {
        let list: Vec<String> = busy.iter().map(|(p, w)| format!("{p} ({w})")).collect();
        bail!(
            "these ports are taken by something other than this stack: {}",
            list.join(", ")
        );
    }
    let dir = stack_dir(&o.home);
    say(&format!("writing the stack to {}", dir.display()));
    write_stack(&dir)?;
    stack_up(&dir)?;

    let (src, name, catalog, password) = match &o.source {
        None => {
            create_sample(&dir)?;
            (
                Source {
                    host: "127.0.0.1".into(),
                    port: 55433,
                    database: SAMPLE_DB.into(),
                },
                o.project.clone().unwrap_or_else(|| SAMPLE_DB.into()),
                SAMPLE_DB.to_string(),
                SAMPLE_PASSWORD.to_string(),
            )
        }
        Some(s) => {
            let src = Source::parse(s)?;
            let password = std::env::var(&o.source_password_env).map_err(|_| {
                anyhow::anyhow!(
                    "set {} to the source password for this install (it is stored in owner-only \
                     files under the EKOS home, never in ekos.toml)",
                    o.source_password_env
                )
            })?;
            let catalog = name_for(&src.database);
            let var = format!("LH_SRC_{}_PASSWORD", catalog.to_ascii_uppercase());
            set_secret(&dir, &var, &password)?;
            std::fs::write(
                dir.join("trino/catalog")
                    .join(format!("{catalog}.properties")),
                catalog_properties(&src, &o.source_user, &var),
            )?;
            restart_trino(&dir)?;
            let name = o.project.clone().unwrap_or_else(|| catalog.clone());
            (src, name, catalog, password)
        }
    };
    if o.source.is_none() {
        restart_trino(&dir)?;
    }
    let project_dir = create_project(o, &name, &src, &catalog, &password)?;
    refresh_projects(o, &name)?;
    start_orchestrator(o, &project_dir)?;
    let console = if o.console { start_console(o)? } else { false };

    println!("\nEKOS Lakehouse is installed.");
    println!("  project      {name}  (ekos-lakehouse --project {name} lakehouse status)");
    println!("  pipeline     {}", project_dir.join("pipeline").display());
    println!("  Trino        http://127.0.0.1:58080");
    println!("  Dagster      http://127.0.0.1:53000");
    if console {
        println!("  console      the Lakehouse tab of project {name}");
    }
    println!("  remove       ekos-lakehouse lakehouse install --uninstall [--purge]");
    Ok(())
}

/// Stop the stack; `purge` also deletes its volumes (the sample database, Iceberg files,
/// Polaris' catalog). Projects and their pipeline repositories stay in the home.
pub fn uninstall(home: &Path, purge: bool) -> Result<()> {
    check_docker()?;
    let dir = stack_dir(home);
    if !dir.join("docker-compose.lakehouse.yml").is_file() {
        bail!("no stack installed under {}", dir.display());
    }
    let mut c = compose(&dir, false);
    c.args([
        "--profile",
        "orchestrator",
        "--profile",
        "observability",
        "down",
        "--remove-orphans",
    ]);
    if purge {
        c.arg("--volumes");
    }
    run(&mut c, "docker compose down")?;
    println!(
        "Stack stopped{}. Projects and pipelines stay in {}; the console is removed with \
         `ekos-lakehouse uninstall`.",
        if purge { " and its data deleted" } else { "" },
        projects_dir(home).display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dagster_workspace_lists_every_installed_project() {
        let home = tempfile::tempdir().unwrap();
        assert!(workspace_yaml(home.path()).is_err(), "no project yet");
        for (name, generated, installed) in [
            ("shop", true, true),
            ("sales", true, true),
            ("half", false, true),
            ("byhand", true, false),
        ] {
            let d = projects_dir(home.path()).join(name);
            if generated {
                std::fs::create_dir_all(d.join("pipeline/dagster")).unwrap();
                std::fs::write(d.join("pipeline/dagster/definitions.py"), "").unwrap();
            }
            if installed {
                std::fs::create_dir_all(d.join(".lakehouse")).unwrap();
                std::fs::write(d.join(".lakehouse/project.json"), "{}").unwrap();
            }
        }
        let y = workspace_yaml(home.path()).unwrap();
        let v: serde_yaml::Value = serde_yaml::from_str(&y).unwrap();
        let names: Vec<&str> = v["load_from"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|l| l["python_file"]["location_name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["sales", "shop"]);
    }

    #[test]
    fn a_source_is_host_port_database_and_never_carries_credentials() {
        let s = Source::parse("db.internal:6543/sales").unwrap();
        assert_eq!(
            (s.host.as_str(), s.port, s.database.as_str()),
            ("db.internal", 6543, "sales")
        );
        assert_eq!(Source::parse("db/sales").unwrap().port, 5432);
        assert!(Source::parse("user:pw@db:5432/sales").is_err());
        assert!(Source::parse("db:5432").is_err());
    }

    #[test]
    fn trino_reaches_this_machine_through_docker() {
        assert_eq!(
            Source::parse("localhost:5432/x").unwrap().for_trino(),
            "host.docker.internal:5432"
        );
        assert_eq!(Source::parse("pg:5/x").unwrap().for_trino(), "pg:5");
    }

    #[test]
    fn the_catalog_names_the_variable_never_the_password() {
        let s = Source::parse("127.0.0.1:5432/Sales-DB").unwrap();
        let name = name_for(&s.database);
        assert_eq!(name, "sales_db");
        let props = catalog_properties(&s, "reader", "LH_SRC_SALES_DB_PASSWORD");
        assert!(props.contains("connection-password=${ENV:LH_SRC_SALES_DB_PASSWORD}"));
        assert!(props.contains("jdbc:postgresql://host.docker.internal:5432/Sales-DB"));
        assert!(props.contains("unsupported-type-handling=CONVERT_TO_VARCHAR"));
        assert_eq!(name_for("2024"), "db_2024");
    }

    #[test]
    fn project_settings_name_a_password_file() {
        let s = Source::parse("127.0.0.1:55433/shop").unwrap();
        let t = project_settings(
            &s,
            "shop",
            "ekos",
            ".lakehouse/source-password",
            &["public".into()],
        );
        let v: toml::Value = toml::from_str(&t).unwrap();
        let l = &v["extensions"]["lakehouse"];
        assert_eq!(
            l["source-password-file"].as_str(),
            Some(".lakehouse/source-password")
        );
        assert_eq!(l["source"].as_str(), Some("127.0.0.1:55433"));
        assert_eq!(l["schemas"][0].as_str(), Some("public"));
        assert!(!t.contains("ekos-local-only"));
    }

    #[test]
    fn the_stack_is_written_with_its_script_executable_and_secrets_private() {
        let dir = tempfile::tempdir().unwrap();
        write_stack(dir.path()).unwrap();
        assert!(dir.path().join("docker-compose.lakehouse.yml").is_file());
        assert!(dir.path().join("trino/catalog/shop.properties").is_file());
        set_secret(dir.path(), "LH_SRC_X_PASSWORD", "a").unwrap();
        set_secret(dir.path(), "LH_SRC_X_PASSWORD", "b").unwrap();
        set_secret(dir.path(), "LH_SRC_Y_PASSWORD", "c").unwrap();
        let env = std::fs::read_to_string(dir.path().join("secrets.env")).unwrap();
        assert_eq!(env, "LH_SRC_X_PASSWORD=b\nLH_SRC_Y_PASSWORD=c\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &str| {
                std::fs::metadata(dir.path().join(p))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777
            };
            assert_eq!(mode("secrets.env"), 0o600);
            assert_eq!(mode("polaris/create-catalog.sh"), 0o755);
        }
        assert!(set_secret(dir.path(), "K", "a\nb").is_err());
    }
}

# Installing EKOS Lakehouse

One command installs everything on one machine: the `ekos-lakehouse` program, the Docker stack
(PostgreSQL sample, Trino, Polaris, SeaweedFS, Dagster), a first project, and the EKOS web
console with its **Lakehouse** tab. Design: [RFC 0199](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0199-lakehouse-console-installer.md).

## What you need

- Linux x86_64 (the tested platform; macOS and Windows: [build from source](#from-source)).
- Docker with Compose v2, usable by your user (`docker ps` works without `sudo`).
- About 6 GB of free memory and 5 GB of disk for the images.
- Ports 55433, 58080, 58181, 58182, 58333 and 53000 free on 127.0.0.1, and 8000 for the console.

## The sample, in one command

```bash
curl -fsSL https://raw.githubusercontent.com/alexeyban/ekos-lakehouse/main/install.sh | sh
```

Prefer to read it first (it is short):

```bash
curl -fsSL https://raw.githubusercontent.com/alexeyban/ekos-lakehouse/main/install.sh -o install.sh
less install.sh && sh install.sh
```

It takes 5–15 minutes, most of it downloading images, and ends in the browser on the console:
project **shop** (customers and orders in the stack's own PostgreSQL), bronze and silver
already loaded once and reconciled, Dagster running the pipeline every hour.

## Your own database

```bash
export EKOS_LH_SOURCE_PASSWORD='…'
curl -fsSL https://raw.githubusercontent.com/alexeyban/ekos-lakehouse/main/install.sh \
  | sh -s -- --source db.example.com:5432/sales --source-user reader
```

| Option | |
|---|---|
| `--source HOST:PORT/DATABASE` | your PostgreSQL (14 or later) |
| `--source-user ROLE` | a role that can read it; a read-only role is the guarantee that nothing writes |
| `--source-password-env VAR` | the variable holding its password during the install (default `EKOS_LH_SOURCE_PASSWORD`) |
| `--name NAME` | the project's name (default: the database's) |
| `--schemas a,b` | only these schemas |
| `--no-console` | no web console |
| `--no-open` | do not open the browser (`ekos-lakehouse console open` later) |
| `--at-login` | start the console when you log in (a systemd user unit) |
| `--no-first-run` | do not run the pipeline once after installing |

Already installed? Run the same command again with another `--source`: the stack is updated in
place and a second project is added. Running it again for an existing project keeps that
project's settings.

**A database on this machine.** Trino runs in Docker, where `localhost` is the container. The
installer writes `host.docker.internal` into Trino's catalog for `localhost`/`127.0.0.1`, which
reaches your machine through Docker's bridge — so PostgreSQL must listen there as well
(`listen_addresses` including `172.17.0.1`, and a `pg_hba.conf` line for `172.17.0.0/16`).
`ekos-lakehouse --project <name> lakehouse doctor` says when Trino cannot see the columns.

## What it changes

Everything is in the EKOS home, `~/.ekos` (or `$EKOS_HOME`):

| Path | What |
|---|---|
| `bin/ekos-lakehouse`, `console/` | the program and the console (replaced on upgrade) |
| `lakehouse/stack/` | the Compose files, Trino catalogs, `secrets.env` (source passwords, mode 600) |
| `lakehouse/projects/<name>/` | the pipeline repository (git), `.lakehouse/source-password` (mode 600), Dagster's state |
| `data/default/<name>/` | the project's `ekos.toml` and its ledger |
| `control/` | the console's settings |

Outside the home: Docker's images, containers and volumes (`ekos-lakehouse_*`); a line adding
`~/.ekos/bin` to `PATH` in your shell profile; with `--at-login`, a systemd user unit.
`ekos.toml` holds no password — only the name of the file that does.

## Several projects

The stack is shared. One Dagster runs one project on its schedule: the first project installed
keeps it, and the installer prints the command that runs another project on demand. The console
shows every project, each with its own Lakehouse tab.

## Offline, from a package

Every release has `ekos-lakehouse-<version>-x86_64-unknown-linux-gnu-full.tar.gz`. Copy it to the
machine, unpack it, and run `./bin/ekos-lakehouse lakehouse install`. The images still come from
Docker Hub; for a machine without internet access, `docker save` them elsewhere and
`docker load` them first (the image list is in `lakehouse/stack/docker-compose.lakehouse.yml`).

## Upgrade

Run the installer again. It replaces `bin/` and `console/`, rewrites the stack's files from the
new binary and recreates the containers whose definition changed. Data volumes, projects and
pipeline repositories are kept. Run `lakehouse generate` in each project afterwards (the console's
**Generate** button) to pick up generator changes.

## Remove

```bash
ekos-lakehouse lakehouse install --uninstall           # stop and remove the stack's containers
ekos-lakehouse lakehouse install --uninstall --purge   # … and its volumes: Iceberg files, catalog, sample
ekos-lakehouse uninstall                               # the console, its login service, the PATH line
```

Projects and their pipeline repositories stay in `~/.ekos` until you delete them.

## From source

```bash
git clone https://github.com/alexeyban/ekos-lakehouse && cd ekos-lakehouse
cargo build --release
./target/release/ekos-lakehouse lakehouse install --no-console
```

A source build has no bundled console; start one from an EKOS checkout (`EKOS_BIN` pointing at
this binary), or follow the step-by-step [quickstart](quickstart.md).

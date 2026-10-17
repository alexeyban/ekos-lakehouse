# Quickstart: your PostgreSQL database to monitored Iceberg tables

From nothing to bronze and silver tables that load on a schedule, reconcile against your source
on every run, and report their health. About 15 minutes, most of it image downloads.

**The one-command install ([install.md](install.md)) does all of this for you.** This page does
it by hand, step by step, from source — to learn what each piece is, or to fit it into a setup of
your own.

Every block marked here is run verbatim by `deploy/e2e/quickstart.sh`, so this page is tested.
The example creates a small database called `shop`; for your own, skip step 2 and use your
database's name and host in step 3.

## What you need

- Docker with Compose v2, and about 6 GB of free memory for the stack.
- Rust 1.98+ and git, to build the binary (a release download will replace this step).
- A PostgreSQL database EKOS can read (PostgreSQL 14+; the sandbox runs 18).

Set two paths for the rest of this page: where you cloned this repository, and an empty
directory for your project.

```bash
export LAKEHOUSE_REPO=$HOME/ekos-lakehouse      # this repository
export MY_PROJECT=$HOME/my-lakehouse            # your project: config, ledger, pipeline
```

## 1. Build the binary and start the stack

<!-- run -->
```bash
cd "$LAKEHOUSE_REPO"
cargo build --release
export PATH="$LAKEHOUSE_REPO/target/release:$PATH"
docker compose -f deploy/docker-compose.lakehouse.yml up -d
```

This starts PostgreSQL (a sandbox source), Trino, Polaris (the Iceberg catalog) and SeaweedFS
(S3 storage). Trino takes about a minute to start.

## 2. A source database (the example)

Skip this with your own database. The example is a shop with customers and orders, and a trigger
that keeps `updated_at` current — which lets EKOS load `orders` incrementally (step 4).

<!-- run -->
```bash
docker exec -i ekos-lakehouse-lh-postgres-1 psql -U ekos -d source -q <<'SQL'
DROP DATABASE IF EXISTS shop;
CREATE DATABASE shop;
SQL
docker exec -i ekos-lakehouse-lh-postgres-1 psql -U ekos -d shop -q <<'SQL'
CREATE TABLE customers (id int PRIMARY KEY, name text NOT NULL, email text, country text);
CREATE TABLE orders (
  id int PRIMARY KEY,
  customer_id int NOT NULL REFERENCES customers (id),
  amount numeric(12,2) NOT NULL,
  status text NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION touch() RETURNS trigger LANGUAGE plpgsql AS
  $$ BEGIN NEW.updated_at := now(); RETURN NEW; END $$;
CREATE TRIGGER touch BEFORE UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION touch();
INSERT INTO customers SELECT i, 'Customer ' || i, 'c' || i || '@example.com', 'DE'
  FROM generate_series(1, 200) i;
INSERT INTO orders SELECT i, 1 + i % 200, (i % 97) * 1.25, 'paid', now()
  FROM generate_series(1, 5000) i;
ANALYZE;
SQL
```

## 3. Let Trino read the source

Trino reads your database through a catalog. **The last two lines are required**: without them
Trino silently leaves out columns of types it does not map (arrays, domains, `tsvector` …), and
EKOS refuses those tables rather than load them incomplete.

<!-- run -->
```bash
cat > "$LAKEHOUSE_REPO/deploy/trino/catalog/shop.properties" <<'EOF'
connector.name=postgresql
connection-url=jdbc:postgresql://lh-postgres:5432/shop
connection-user=ekos
connection-password=${ENV:PG_PASSWORD}
postgresql.array-mapping=AS_ARRAY
unsupported-type-handling=CONVERT_TO_VARCHAR
EOF
docker restart ekos-lakehouse-lh-trino-1
until curl -s http://127.0.0.1:58080/v1/info | grep -q '"starting":false'; do sleep 3; done
```

For your own database, `connection-url` points at it (from inside the Docker network), and
`connection-user` should be a role that can only read.

## 4. The project

A project is a directory with an `ekos.toml`. The password is never written in it: the config
names the environment variable that holds it.

<!-- run -->
```bash
mkdir -p "$MY_PROJECT" && cd "$MY_PROJECT"
export EKOS_LH_PG_PASSWORD=ekos-local-only
cat > ekos.toml <<'EOF'
[observe]
paths = ["pipeline"]

[extensions.lakehouse]
trino-url = "http://127.0.0.1:58080"
polaris-health-url = "http://127.0.0.1:58182/q/health"
s3-url = "http://127.0.0.1:58333"
source = "127.0.0.1:55433"
source-database = "shop"
source-user = "ekos"
source-password-env = "EKOS_LH_PG_PASSWORD"
source-catalog = "shop"
bronze-schedule = "0 * * * *"

[extensions.lakehouse.watermarks]
"public.orders" = "updated_at"
EOF
ekos-lakehouse init
ekos-lakehouse lakehouse doctor
```

`doctor` checks every service, that Trino exposes every column of your tables, and whether the
source role can write (a warning: EKOS only reads, but a read-only role is the guarantee).

## 5. See the plan, then generate the pipeline

<!-- run -->
```bash
ekos-lakehouse lakehouse plan
ekos-lakehouse lakehouse generate
```

`plan` shows each table's load mode — `orders` is incremental because `updated_at` is declared
*and* the `touch` trigger proves it moves on every update; `customers` is loaded in full — and
any column it could not land faithfully. `generate` writes the pipeline repository under
`pipeline/`: the bronze SQL, a dbt project for silver, a Dagster project, and
`lakehouse.lock.json`, the deployed plan. It is a git repository; EKOS commits to it.

## 6. Run it

The orchestrator is Dagster. One run, synchronously, in a one-off container:

<!-- run -->
```bash
export EKOS_LH_PROJECT="$MY_PROJECT"
export EKOS_LAKEHOUSE_BIN="$LAKEHOUSE_REPO/target/release/ekos-lakehouse"
mkdir -p "$MY_PROJECT/.dagster"
cd "$LAKEHOUSE_REPO"
docker compose -f deploy/docker-compose.lakehouse.yml --profile orchestrator run --rm lh-dagster \
  dagster job execute -f "$MY_PROJECT/pipeline/dagster/definitions.py" -j bronze
```

Each table is loaded by Trino into Iceberg, reconciled against your source (row counts,
per-column aggregates, checksums — with a planted difference that must be caught), then silver is
built by dbt with its tests.

## 7. Is it right?

<!-- run -->
```bash
cd "$MY_PROJECT"
ekos-lakehouse lakehouse status
```

Every table, per layer: `healthy`, `late`, `unverified`, `failing` or `schema_pending`, with the
facts behind it. To keep it running on its schedule, start Dagster's server:

```bash
docker compose -f "$LAKEHOUSE_REPO/deploy/docker-compose.lakehouse.yml" --profile orchestrator up -d lh-dagster
# http://127.0.0.1:53000 — the job runs on `bronze-schedule`
```

## Next

- [Runbook](runbook.md): what each state means and what to do.
- [Troubleshooting](troubleshooting.md): what goes wrong, and why.
- Alerts and metrics: set `alert-webhook-env` and `otlp-endpoint` (runbook).
- Source changes: `ekos-lakehouse lakehouse sync` (runs before every load in Dagster).
- Gold marts from written descriptions: `ekos-lakehouse lakehouse gold draft marts/<name>.md`.

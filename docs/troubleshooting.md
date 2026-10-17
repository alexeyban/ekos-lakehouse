# Troubleshooting

Every entry here happened while EKOS Lakehouse was built (devlogs 1–7).

| Symptom | Cause | Fix |
|---|---|---|
| `plan` refuses a table: "the Trino connector does not expose this column" | the source catalog lacks `postgresql.array-mapping=AS_ARRAY` and `unsupported-type-handling=CONVERT_TO_VARCHAR`; without them Trino **silently** drops columns of types it does not map | add both to the catalog's `.properties`, restart Trino; `doctor` checks it |
| `plan` refuses an unconstrained `numeric` | Trino reads it as `number`, which Iceberg cannot store | EKOS measures the precision and scale used; NaN or ±Infinity values (or a number wider than 38 digits) leave it refused — give the column a precision |
| `plan` refuses a timestamp: "hold infinity or -infinity" | Trino reads `'infinity'` as a wrong finite date (year -171979), without an error | map infinity in a source view, or leave the table out |
| `DROP TABLE` fails: "Unable to purge entity" | Polaris refuses Trino's purge unless the catalog allows it | the sandbox catalog sets `polaris.config.drop-with-purge.enabled`; set it on yours |
| loads fail with `HIVE_WRITER_CLOSE_ERROR` / `UnknownHostException: lh-seaweedfs` | the S3 store was OOM-killed under parallel uploads | 1 GB for SeaweedFS (the sandbox default); `docker inspect … OOMKilled` |
| Trino reports healthy but queries fail | `/v1/info` answers 200 while `"starting": true` | wait for `"starting":false` (the healthcheck and `doctor` do) |
| `reconcile` reports a V3 "value it cannot render canonically" | one side returned NULL where a canonical value was expected — usually a misread value (above) | the table is diverged; find the column from V2 |
| `sync`: "running git (is it installed and on PATH?)" | `git` missing where `sync` runs | the sandbox Dagster image has it; install it on your orchestrator |
| several Dagster assets fail "Namespace already exists" | `CREATE SCHEMA IF NOT EXISTS` races on an Iceberg REST catalog | handled since devlog 3; regenerate an older pipeline |
| "another writable process already holds the ledger's write lock" | parallel `reconcile`/`run-report` | they wait up to 2 minutes; persistent → an `ekos` process stuck on the project |
| the Dagster code location fails to load: "depends on a source … not found" | a stale generated model | `generate` removes files of tables that left the plan; regenerate |
| `gold draft` refuses: "commit marts/… first" | drafting needs a committed description | commit it |
| a gold term comes back as a question | the term names nothing in silver, or several things | answer in the description (write `table.column`), commit, draft again |
| a gold draft "was rejected by the engine" | the model's SQL is not valid Trino | it retries with the message; rewrite the term as a pattern (`sum of …`) if it keeps failing |
| `cargo build` cannot resolve `zip 2.x` | every `zip 2.x` release is yanked | keep the committed `Cargo.lock` (seeded from EKOS's) |
| a new table has no Dagster asset after `sync` | Dagster loaded the definitions before the table existed | reload the code location (restart `lh-dagster`) |
| Claude Code / builds killed for memory | the stack (~4 GB) + Dagster (1 GB) + a desktop on 16 GB | stop Dagster between runs; `docker compose … down` when idle |

## The installer and the console (RFC 0199)

**"these ports are taken by something other than this stack"** — another program holds one of
the stack's loopback ports. The stack's own containers never count (a second run updates them).
Stop the other program, or install on a machine where the ports are free.

**`Docker Compose v2 failed`** — `docker compose version` must work for your user. The Compose
plugin is looked up in `~/.docker/cli-plugins` (or `$DOCKER_CONFIG`); a package-manager install
of `docker-compose-plugin` puts it where every user finds it.

**Doctor in the console: `columns … source` fails, "names this connection's password but is not
set"** — the project was set up by hand with `source-password-env` only. Console jobs do not
inherit your shell's variables. Add `source-password-file = ".lakehouse/source-password"` to the
project's `[extensions.lakehouse]` and put the password in that file with mode 600 (the installer
does both).

**"the password file … readable by others"** — `chmod 600` it. A password file anyone else on
the machine can read is refused rather than used.

**A database on this machine is not seen by Trino** — Trino reaches it as `host.docker.internal`
through Docker's bridge; PostgreSQL must listen on `172.17.0.1` too and allow `172.17.0.0/16` in
`pg_hba.conf` ([install.md](install.md#your-own-database)).

**"warning: project X left out of Dagster"** — re-rendering that project's pipeline from its
deployed plan failed (the message says why). The other projects run; fix the cause and run the
installer again, or `ekos-lakehouse --project X lakehouse generate --from-lock`.

**The Lakehouse tab is missing** — the console was started by the public `ekos` binary, which has
no Lakehouse pages. Start it with `ekos-lakehouse console start` (the installer does); a
restricted reader (a read scope) never sees the tab.

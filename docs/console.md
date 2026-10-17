# The Lakehouse tab of the web console

The installer sets up the EKOS web console; every Lakehouse project has a **Lakehouse** tab next
to EKOS's own (Dashboard, Graph, Chat …). The page is described by `ekos-lakehouse` itself and
drawn by the console ([RFC 0198](https://github.com/alexeyban/EKOS/blob/main/ekos/docs/rfcs/0198-console-extension-pages.md)):
each panel shows what one read-only command prints, each button runs one command as a job.

```bash
ekos-lakehouse console open        # signed in with a one-time code (local mode)
```

## Panels

| Panel | Command | What it says |
|---|---|---|
| Health per table and layer | `lakehouse status --json` | `healthy`, `late`, `unverified`, `failing` or `schema_pending` per table and layer, with the fact that put it there and, for a failure, the probable cause ([runbook](runbook.md)) |
| Waiting for a merge | `lakehouse proposals --json` | schema changes and gold marts proposed as branches of the pipeline repository; merging one is the approval |
| Tables and how they load | `lakehouse tables --json` | each table's load mode (`full`, or `incremental` with its watermark), key, and the columns not landed exactly (widened, kept as text) |
| Tables not loaded | `lakehouse tables --json` | tables refused, and why (a column with no faithful Iceberg type) |
| Stack | `lakehouse tables --json` | where Trino, Dagster, Polaris and storage are; the source; the schedule; the pipeline repository |

Each panel has **refresh**; nothing refreshes by itself.

## Buttons

| Button | Command | Class | Who |
|---|---|---|---|
| Doctor | `lakehouse doctor` | external | admin, with the command typed to confirm |
| Sync source changes | `lakehouse sync` | external | admin, typed confirmation |
| Check the pipeline | `lakehouse check` | write | operator |
| Plan | `lakehouse plan` | external | admin, typed confirmation |
| Generate | `lakehouse generate` | external | admin, typed confirmation |

A button opens the run's page with its log. *External* commands reach outside the EKOS home (the
database, Trino), which is why they need an admin and a typed confirmation. A viewer sees the
panels and the buttons greyed out. Someone with access to only part of a project (a read scope)
does not see the tab: its commands read the whole project.

## Credentials in a team console

The installer's local console runs as you, and the project reads its source password from
`.lakehouse/source-password`. A team console (OIDC, several tenants) starts each job with only
its tenant's granted variables ([EKOS deployment guide](https://github.com/alexeyban/EKOS/blob/main/docs/deployment.md#7-model-credentials)):
keep the password file in the project, or grant the variable `source-password-env` names to the
tenant.

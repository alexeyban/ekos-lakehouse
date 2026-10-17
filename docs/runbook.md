# Runbook

What `ekos-lakehouse lakehouse status` (and the alerts, the metrics, the MCP tools — they all
read the same ledger facts) tells you, and what to do about it.

## The states

One state per table and layer, worst first:

| State | Means | Do |
|---|---|---|
| `schema_pending` | the source changed and the change waits on a proposal branch (`ekos/change/<table>/<id>`); while it is open, a rename, drop or narrowing **pauses** the table's loads, other changes let them continue on the deployed schema | read `proposals/<id>.md` on the branch (the change, the evidence, the silver columns and tests it reaches); merge it to approve; the next `sync` resumes the table |
| `failing` | the last load or build failed, or its reconciliation **diverged** from the source | read the evidence and the probable cause (a recent source change, or the data). A divergence names the tier: V1 count, V2 a column's aggregate, V3 a bucket. `lakehouse reconcile --table <t> --json` repeats it |
| `unverified` | the tiers agreed but the planted control did not fire — the comparison could not have caught a difference | treat it as broken monitoring, not as healthy; usually a table whose compared columns all lack a canonical rule |
| `late` | no successful run within `late-after-minutes` (default 120) | is the orchestrator running? `docker compose … --profile orchestrator ps`; Dagster's run log |
| `healthy` | the last run succeeded and reconciled, with the control fired | — |

## Alerts

```toml
[extensions.lakehouse]
alert-webhook-env = "EKOS_LH_ALERT_WEBHOOK"   # the variable holding the URL (a URL can hold a token)
late-after-minutes = 120
```

`lakehouse alert` posts each **transition** once (`{"text": …, "table", "layer", "state",
"previous", "evidence", "cause"}` — Slack, Teams and PagerDuty's generic webhook take `text`),
and remembers the last states in `pipeline/.lakehouse/alerts.json`. Dagster's `lakehouse/health`
asset runs it after every run.

## Metrics

```toml
otlp-endpoint = "http://127.0.0.1:54318"     # an OpenTelemetry collector
```

`lakehouse metrics` pushes `lakehouse.table.state` (0 healthy … 4 schema pending),
`lakehouse.table.freshness` (s), `lakehouse.table.rows_written`, `lakehouse.table.run_duration`
(ms), labelled `table` and `layer`. The sandbox collector (`--profile observability`) re-exposes
them for Prometheus at `:58889/metrics`.

## Agents

`ekos-lakehouse mcp serve --workspace <project>` serves `ekos_pipeline_status` and
`ekos_table_health` beside EKOS's own tools. Read-only: no tool reruns, merges or approves.

## Routine work

| Task | How |
|---|---|
| A source change | `lakehouse sync` (Dagster runs it before every load): safe changes are committed, the rest proposed |
| A new gold mart | commit `marts/<name>.md`, then `lakehouse gold draft marts/<name>.md` |
| Code quality | `lakehouse check --lint` after compiling the repository with EKOS (`build` … `commit`) |
| Iceberg maintenance | scheduled (`maintenance-schedule`, default 03:00): compaction, snapshot expiry (7 days), orphan removal |
| A hand edit to a generated file | kept and reported as drift; `generate` works around it |

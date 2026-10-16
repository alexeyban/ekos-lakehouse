# Devlog 2 — M0 exit met: the sandbox stack, Trino on the read path, V3 catches a planted defect

**Date:** 2026-10-16
**PRs:** none (local)
**Branch:** main

---

## Summary

The whole data plane runs from one compose file. A read-only Trino client implements EKOS's
`EngineReader`, and the canonical-hash SQL EKOS generates for Trino (EKOS devlog_274) agrees
with the golden table and the Rust implementation on a live Trino: 31 of 31 values, hashes,
prefixes and bucket checksums. A PostgreSQL table copied to Iceberg reconciles at V1–V3, and
one changed value in 2,000 rows fails V3 while V1 stays green: **the RFC 0190 M0 exit**. The
load path was measured: 0.46–0.91 M rows/s for 10 M rows. Three real stack problems were
found and fixed on the way.

---

## The stack (`deploy/docker-compose.lakehouse.yml`)

| Service | Image | Port (loopback) | Cap |
|---|---|---|---|
| source PostgreSQL | `postgres:16.15-alpine` (pg_stat_statements, logical WAL, `LANG=C`, UTC) | 55433 | 512m |
| S3 | `chrislusf/seaweedfs:4.48` + `amazon/aws-cli:2.36.44` bucket job | 58333 | 1g |
| catalog | `apache/polaris:1.8.0` + `alpine/curl:8.22.0` setup job | 58181 / 58182 | 1g |
| engine | `trinodb/trino:483` (`postgresql` + `iceberg` REST catalogs) | 58080 | 2g |
| orchestrator | `ekos-lakehouse/dagster:1.13.26` (dagster-dbt 0.29.26, dbt-trino 1.10.6) | 53000 | 1g |

### Decisions
- **SeaweedFS, not MinIO.** MinIO's community edition stopped publishing images in October 2025
  and is unmaintained; `minio/minio` no longer pulls. The user chose SeaweedFS (Apache-2.0,
  maintained). Nothing depends on the store: an endpoint, path-style access, keys. The doctor's
  S3 check accepts any non-5xx answer, since stores differ in health endpoints and an anonymous
  request is refused with 403 — which still proves the store is up.
- **No credential vending.** SeaweedFS has no usable STS here: the catalog sets
  `stsUnavailable`, Trino uses static keys (`vended-credentials-enabled=false`).
- **Trino 483, not 484**: 484 was released the same day.
- Dagster is built but not started for M0's tests (memory: the machine had 1–4 GB free).

---

## Trino client (`crates/trino`)

| Piece | What |
|---|---|
| `Client` | `POST /v1/statement`, follow `nextUri`, collect `columns`/`data`, stop on `error`; retry 502–504 |
| `TrinoReader` | `EngineReader` for `Dialect::Trino`; NULL as an empty cell (as the PostgreSQL reader); decimals keep their scale (Trino sends them as strings) |
| `check_read_only` | every statement classified with EKOS's RFC 0160 classifier (`classify_with`, Trino grammar) before it is sent; anything but one plain read is refused — the validator cannot change what it validates |
| `tests/live.rs` | `EKOS_LAKEHOUSE_LIVE=1`: 31 hand-written Trino literals vs the golden forms; hashes/prefixes vs Rust; bucket checksums vs Rust; the M0 exit test. PostgreSQL is read directly, never through Trino (independent oracle) |
| `examples/throughput.rs` | the load measurement below |

---

## Load throughput (M0 measurement)

10,000,000 rows, 6 columns (bigint, text, numeric(14,4), timestamp, boolean, int), 1.2 GB in
PostgreSQL, loaded `postgresql` → `iceberg` by Trino. Three runs on the sandbox (laptop, 12
cores, Trino capped at 2 GB, the host swapping):

| Statements | Time | Rows/s |
|---|---|---|
| 1 `CREATE TABLE AS` | 17.4–21.8 s | 0.46–0.57 M |
| 2 key ranges in parallel | 13.3 s | 0.75 M |
| 4 key ranges in parallel | 11.0–12.7 s | 0.79–0.91 M |
| 8 key ranges in parallel | 17.0–23.8 s | 0.42–0.59 M |

**Decision: Trino stays the load path**; the Spark fallback is not needed for M1. Four ranges is
the sandbox's best; the generator should make the split count a setting, defaulting to 4, and
the production number has to be measured on production-sized hardware.

---

## Knowledge Captured

- **An unconstrained PostgreSQL `numeric` reaches Trino 483 as type `number`, which Iceberg
  rejects** (`Type not supported for Iceberg: number`). The M1 type map must decide every
  unconstrained numeric explicitly (measure the scale used, or refuse) — it cannot pass through.
- **Polaris refuses `DROP TABLE` from Trino** unless the catalog sets
  `polaris.config.drop-with-purge.enabled`: Trino's Iceberg REST connector always drops with
  purge. The first live run passed only because its tables did not exist yet; the second run
  would have failed. Set in `create-catalog.sh`.
- **SeaweedFS all-in-one (`weed server -s3`) is OOM-killed at 512 MB** under four or more
  parallel Parquet uploads. The failure surfaced in Trino as `HIVE_WRITER_CLOSE_ERROR` /
  `UnknownHostException: lh-seaweedfs` — the container was gone. 1 GB + `restart: unless-stopped`.
- **Trino's `/v1/info` answers 200 while `starting: true`**, so a healthcheck on the status code
  says healthy too early. The doctor and the compose healthcheck both test `"starting":false`.
- **`trinodb/trino` sizes its heap at 80% of the container limit** (`MaxRAMPercentage=80`); a
  hand-written `jvm.config` is unnecessary and stale options stop the JVM
  (`OmitStackTraceInLastException`).
- **Polaris in the sandbox keeps its catalog in memory**: recreating the container loses every
  table registration (files stay in S3, orphaned).

---

## Files Changed

| File | Change summary |
|---|---|
| `deploy/docker-compose.lakehouse.yml` + `deploy/{polaris,seaweedfs,trino,dagster}/` | the stack |
| `crates/trino/` | client, read-only reader, live suite, throughput example |
| `crates/lakehouse-ext/src/{doctor,settings}.rs` | store-neutral `s3-url` |
| `README.md` | S3 store, ports |

# Gold marts on DuckDB (EKOS RFC 0259): dbt-duckdb reads silver from the Iceberg catalog and builds
# every mart into marts.next.duckdb. `lakehouse gold publish` runs that build with its tests,
# reconciles each measure with silver in Trino, records each mart's run (RFC 0196/0258 health and
# freshness), and only then replaces the published marts file. A failure leaves it as it was.
GOLD_DUCKDB_DIR = REPO / "gold-duckdb"


def _duckdb_marts():
    models = GOLD_DUCKDB_DIR / "models" / "gold"
    if not models.is_dir():
        return []
    return sorted(p.stem for p in models.glob("gold__*.sql"))


_DUCKDB_MARTS = _duckdb_marts() if (GOLD_DUCKDB_DIR / "dbt_project.yml").exists() else []
if _DUCKDB_MARTS:
    # After silver: every silver model's asset, when the dbt-trino project is there.
    _SILVER_KEYS = [
        k for a in assets if a is not sync and getattr(a, "keys", None)
        for k in a.keys if "bronze" not in k.path
    ]

    @dg.multi_asset(
        name="gold_duckdb",
        group_name="gold_duckdb",
        can_subset=True,
        specs=[
            dg.AssetSpec(
                key=[*PREFIX, "gold_duckdb", m],
                deps=_SILVER_KEYS,
                description=f"{m} in the DuckDB marts file (EKOS RFC 0259)",
            )
            for m in _DUCKDB_MARTS
        ],
    )
    def gold_duckdb(context: dg.AssetExecutionContext):
        # The marts file is one file: every mart is built, tested and reconciled together, and
        # published together or not at all.
        result = _ekos("lakehouse", "gold", "publish", "--run-id", context.run_id)
        context.log.info(result.stdout + result.stderr)
        if result.returncode != 0:
            raise dg.Failure(
                description="gold marts on DuckDB were not published; the published file is unchanged",
                metadata={"publish": dg.MetadataValue.text(result.stdout + result.stderr)},
            )
        for key in context.selected_asset_keys:
            yield dg.MaterializeResult(
                asset_key=key,
                metadata={"publish": dg.MetadataValue.text(result.stdout.strip())},
            )

    assets.append(gold_duckdb)



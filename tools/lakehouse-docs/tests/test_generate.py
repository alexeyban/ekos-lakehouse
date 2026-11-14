import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest

from conftest import FIX
from lakehouse_docs import lineage, project
from lakehouse_docs.generate import generate
from lakehouse_docs.lineage import Node

EXPECTED = [
    "index.md", "business/index.md", "business/glossary.md", "architecture/index.md",
    "architecture/decisions.md", "source/index.md", "source/er-diagram.md",
    "source/tables/public.customer.md", "target/index.md", "target/bronze.md",
    "target/silver/index.md", "target/silver/er-diagram.md", "target/silver/tables/public__payment.md",
    "target/gold/index.md", "target/gold/marts/rentals_by_customer_month.md", "lineage/index.md",
    "lineage/column-lineage.md", "quality/index.md", "quality/reconciliation.md",
    "quality/data-tests.md", "quality/code-quality.md", "quality/health.md",
    "governance/data-dictionary.md", "governance/pii-and-security.md", "operations/runbook.md",
    "operations/sla.md", "operations/schema-changes.md", "operations/changelog.md",
]


def test_every_page_is_written(docs):
    for rel in EXPECTED:
        assert (docs / rel).exists(), rel
        assert (docs / rel).read_text().startswith("---\ntitle: ")


def test_internal_links_resolve(docs):
    for page in docs.rglob("*.md"):
        for target in re.findall(r"\]\(([^)#]+\.md)(?:#[^)]*)?\)", page.read_text()):
            assert (page.parent / target).resolve().exists(), f"{page}: {target}"


@pytest.mark.skipif(shutil.which("mmdc") is None, reason="mermaid-cli not installed")
def test_every_mermaid_diagram_renders(docs):
    blocks = [b for p in docs.rglob("*.md") for b in re.findall(r"```mermaid\n(.*?)```", p.read_text(), re.S)]
    assert len(blocks) >= 10
    for b in blocks:
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "a.mmd").write_text(b)
            r = subprocess.run(["mmdc", "-i", f"{d}/a.mmd", "-o", f"{d}/a.svg", "-q"], capture_output=True, text=True)
            assert r.returncode == 0 and (Path(d) / "a.svg").exists(), b[:300] + r.stderr[-300:]


def test_column_lineage_reaches_source_through_every_layer(pipeline):
    p = project.load(pipeline, FIX / "evidence")
    lin = lineage.build(p)
    assert lin.unresolved == []
    roots = lin.roots(Node("gold", "gold__rentals_by_customer_month", "revenue"))
    assert [n.label for n, _ in roots] == ["public.payment.amount"]
    month = lin.roots(Node("gold", "gold__rentals_by_customer_month", "month"))
    assert month[0][0].label == "public.rental.rental_date"
    assert "DATE_TRUNC" in month[0][1][-1].transform
    email = lin.up[Node("silver", "public__customer", "email")][0]
    assert "HMAC_SHA256" in email.transform


def test_column_lineage_matches_upper_case_oracle_names(pipeline):
    """sqlglot's Trino dialect lower-cases identifiers, quoted ones too; an Oracle pipeline's models
    and columns are upper case. Gold must still reach the silver column it reads."""
    p = project.load(pipeline, FIX / "evidence")
    silver = project.Model(
        name="ADEMPIERE__C_ORDER", layer="silver", path="x.sql", sql='select "C_ORDER_ID", "GRANDTOTAL" from t',
        columns=[project.ModelColumn("C_ORDER_ID"), project.ModelColumn("GRANDTOTAL")],
    )
    gold = project.Model(
        name="gold__orders", layer="gold", path="y.sql",
        sql='select sum("C_ORDER"."GRANDTOTAL") as "total"\nfrom {{ ref(\'ADEMPIERE__C_ORDER\') }} as "C_ORDER"',
        columns=[project.ModelColumn("total")], refs=["ADEMPIERE__C_ORDER"],
    )
    p.models[silver.name] = silver
    p.models[gold.name] = gold
    lin = lineage.build(p)
    assert "gold__orders.total: no upstream column" not in lin.unresolved
    [edge] = lin.up[Node("gold", "gold__orders", "total")]
    assert edge.src == Node("silver", "ADEMPIERE__C_ORDER", "GRANDTOTAL")
    assert edge.kind == "parsed"


def test_er_diagram_has_declared_relations(docs):
    er = (docs / "source/er-diagram.md").read_text()
    assert 'public_payment }o--|| public_rental : "rental_id"' in er
    assert "integer customer_id PK" in er
    silver = (docs / "target/silver/er-diagram.md").read_text()
    assert "public__payment }o--|| public__customer" in silver


def test_evidence_shows_up(docs):
    q = (docs / "quality/index.md").read_text()
    assert "6 green, 1 diverged, 1 unverified" in q
    assert "7/8 passed" in q
    assert "❌ fail (1 rows)" in (docs / "target/silver/tables/public__payment.md").read_text()
    assert "schema pending" in (docs / "target/silver/index.md").read_text()


def test_masking_is_reported_from_the_sql(docs):
    pii = (docs / "governance/pii-and-security.md").read_text()
    assert "| `public.customer.email` | email | hmac |" in pii
    assert "| `public.address.phone` | phone | null |" in pii
    assert "⚠️ not masked" in pii


def test_manual_sections_survive_and_reruns_are_noops(pipeline, tmp_path):
    out = tmp_path / "d"
    generate(pipeline, out, FIX / "evidence", None)
    page = out / "source/tables/public.customer.md"
    page.write_text(page.read_text().replace("_Business meaning of this table, known quirks, data owner._", "Owned by CRM."))
    res = generate(pipeline, out, FIX / "evidence", None)
    assert "Owned by CRM." in page.read_text()
    assert res.written == []


def test_stale_generated_pages_are_removed_and_hand_pages_kept(pipeline, tmp_path):
    out = tmp_path / "d"
    generate(pipeline, out, FIX / "evidence", None)
    (out / "notes.md").write_text("# My notes\n")
    (out / "source/tables/public.gone.md").write_text('---\ntitle: "x"\nekos_generated: true\n---\n')
    res = generate(pipeline, out, FIX / "evidence", None)
    assert "source/tables/public.gone.md" in res.removed
    assert (out / "notes.md").exists()


def test_without_evidence_quality_pages_say_so(pipeline, tmp_path):
    out = tmp_path / "d"
    res = generate(pipeline, out, tmp_path / "none", None)
    assert "No run evidence collected yet" in (out / "quality/index.md").read_text()
    assert any("no run evidence" in w for w in res.warnings)


def test_plan_settings_reach_the_pages(docs):
    # The lock's settings are kebab-case; every page that names them must show the values.
    home = (docs / "index.md").read_text()
    assert "| Source catalog (Trino) | `pagila` |" in home, home
    assert "| Target catalog (Iceberg) | `iceberg` |" in home
    runbook = (docs / "operations/runbook.md").read_text()
    assert "| Bronze loads | `0 * * * *` |" in runbook, runbook
    assert "| Iceberg maintenance | `0 3 * * *` |" in runbook
    assert "namespace `bronze_<schema>`" in (docs / "architecture/index.md").read_text()


def test_a_duckdb_mart_is_parsed_in_duckdbs_dialect(pipeline, tmp_path):
    """EKOS RFC 0259 L7: a gold mart of the dbt-duckdb project reads silver as sources named after
    the silver models; its lineage is parsed with sqlglot's DuckDB dialect and still reaches the
    source column through every layer."""
    trino_sql = (pipeline / "dbt/models/gold/gold__rentals_by_customer_month.sql").read_text()
    duck = pipeline / "gold-duckdb/models/gold"
    duck.mkdir(parents=True)
    duck_sql = re.sub(r"\{\{ ref\('(public__\w+)'\) \}\}", r"{{ source('silver_public', '\1') }}", trino_sql)
    # DuckDB-only syntax: a `::` cast and GROUP BY ALL.
    duck_sql = duck_sql.replace('sum("payment"."amount")', 'sum("payment"."amount")::decimal(18, 2)')
    duck_sql = duck_sql.replace("group by 1, 2, 3", "group by all")
    (duck / "gold__rentals_by_customer_month.sql").write_text(duck_sql)
    (pipeline / "dbt/models/gold/_gold__rentals_by_customer_month.yml").rename(
        duck / "_gold__rentals_by_customer_month.yml")
    (pipeline / "dbt/models/gold/gold__rentals_by_customer_month.sql").unlink()

    p = project.load(pipeline, FIX / "evidence")
    mart = p.models["gold__rentals_by_customer_month"]
    assert mart.engine == "duckdb" and mart.layer == "gold"
    assert "public__payment" in mart.refs and mart.sources == []
    lin = lineage.build(p)
    assert lin.dialects["gold__rentals_by_customer_month"] == "duckdb"
    assert lin.dialects["public__payment"] == "trino"
    assert lin.unresolved == []
    assert not [f for f in lin.fallbacks if f.startswith("gold__")], lin.fallbacks
    roots = lin.roots(Node("gold", "gold__rentals_by_customer_month", "revenue"))
    assert [n.label for n, _ in roots] == ["public.payment.amount"]
    month = lin.roots(Node("gold", "gold__rentals_by_customer_month", "month"))
    assert month[0][0].label == "public.rental.rental_date"

    out = tmp_path / "docs-duck"
    generate(pipeline, out, FIX / "evidence", FIX / "lakehouse-docs.toml")
    page = (out / "lineage/index.md").read_text()
    assert "parsed in DuckDB's dialect" in page

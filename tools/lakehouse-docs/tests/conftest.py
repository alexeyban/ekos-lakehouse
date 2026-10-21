import shutil
import subprocess
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "publish"))

FIX = HERE / "fixtures"


def _git(cwd, *args):
    subprocess.run(["git", "-c", "user.email=t@t", "-c", "user.name=ekos", *args], cwd=cwd, check=True, capture_output=True)


@pytest.fixture
def pipeline(tmp_path):
    """The fixture pipeline repository (lock + silver from the real generators, a gold mart), as git."""
    p = tmp_path / "pipeline"
    shutil.copytree(FIX / "pipeline", p)
    _git(p, "init", "-q")
    _git(p, "add", "-A")
    _git(p, "commit", "-qm", "lakehouse: generate")
    (p / "dbt/models/gold/gold__rentals_by_customer_month.sql").write_text(
        (p / "dbt/models/gold/gold__rentals_by_customer_month.sql").read_text() + "-- touched\n")
    _git(p, "commit", "-qam", "lakehouse: sync — applied new-nullable-column public.film.original_language_id")
    return p


@pytest.fixture
def docs(pipeline, tmp_path):
    from lakehouse_docs.generate import generate
    out = tmp_path / "docs"
    generate(pipeline, out, FIX / "evidence", FIX / "lakehouse-docs.toml")
    return out

"""Collect run evidence by calling ekos-lakehouse's own ``--json`` commands, and copy dbt's
run artifacts. Everything lands in one directory the generator reads (default
``<pipeline>/docs/_evidence``).

Read-only by default: ``status``, ``tables`` and ``proposals`` only read the ledger and git.
``--check`` runs ``lakehouse check --lint`` (it records its metrics in the ledger, as every check
run does). ``--reconcile`` runs ``lakehouse reconcile``, which queries the source and Trino and
plants a control row — run it when a fresh reconciliation is wanted, not on every doc build.
"""

from __future__ import annotations

import datetime as dt
import json
import shutil
import subprocess
from pathlib import Path


def _run(binary: str, project: Path, args: list[str]) -> tuple[object | None, str]:
    try:
        cp = subprocess.run([binary, *args], cwd=project, capture_output=True, text=True, timeout=3600)
    except FileNotFoundError:
        return None, f"{binary} not found on PATH (pass --binary)"
    except subprocess.TimeoutExpired:
        return None, "timed out"
    text = cp.stdout.strip()
    # `check` and `reconcile` exit non-zero when something is not met, and still print JSON.
    start = min([i for i in (text.find("["), text.find("{")) if i >= 0], default=-1)
    if start < 0:
        return None, (cp.stderr.strip() or f"exit {cp.returncode}, no JSON")[:500]
    try:
        return json.loads(text[start:]), "" if cp.returncode == 0 else f"exit {cp.returncode}"
    except json.JSONDecodeError as e:
        return None, f"invalid JSON: {e}"


def collect(project: Path, pipeline: Path, out: Path, binary: str = "ekos-lakehouse",
            check: bool = False, reconcile: bool = False, tables: list[str] | None = None) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    report: dict[str, str] = {}
    jobs = [
        ("status", ["lakehouse", "status", "--json"]),
        ("tables", ["lakehouse", "tables", "--json"]),
        ("proposals", ["lakehouse", "proposals", "--json"]),
    ]
    if check:
        jobs.append(("check", ["lakehouse", "check", "--lint", "--json"]))
    if reconcile:
        args = ["lakehouse", "reconcile", "--json"]
        for t in tables or []:
            args += ["--table", t]
        jobs.append(("reconcile", args))
    for name, args in jobs:
        data, err = _run(binary, project, args)
        if data is not None:
            (out / f"{name}.json").write_text(json.dumps(data, indent=1), encoding="utf-8")
        report[name] = "ok" + (f" ({err})" if err else "") if data is not None else f"skipped: {err}"

    target = pipeline / "dbt" / "target"
    for f in ("run_results.json", "manifest.json", "catalog.json"):
        if (target / f).exists():
            shutil.copy2(target / f, out / f)
            report[f] = "copied"
    head = subprocess.run(["git", "-C", str(pipeline), "rev-parse", "--short", "HEAD"], capture_output=True, text=True)
    meta = {
        "collected_at": dt.datetime.now(dt.UTC).strftime("%Y-%m-%d %H:%M UTC"),
        "head": head.stdout.strip() or None,
        "report": report,
    }
    (out / "collected.json").write_text(json.dumps(meta, indent=1), encoding="utf-8")
    return meta

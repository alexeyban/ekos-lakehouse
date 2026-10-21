"""python -m lakehouse_docs {collect,generate,all} …"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from .collect import collect
from .generate import generate


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="lakehouse_docs", description="Generate EKOS Lakehouse documentation.")
    sub = ap.add_subparsers(dest="cmd", required=True)

    def common(p: argparse.ArgumentParser) -> None:
        p.add_argument("--project", type=Path, default=Path("."), help="EKOS project directory (holds ekos.toml); default: .")
        p.add_argument("--pipeline", type=Path, help="pipeline repository; default: <project>/pipeline")
        p.add_argument("--evidence", type=Path, help="evidence directory; default: <out>/_evidence")
        p.add_argument("--out", type=Path, help="docs output directory; default: <pipeline>/docs")

    g = sub.add_parser("generate", help="write Markdown + Mermaid pages")
    common(g)
    g.add_argument("--config", type=Path, help="lakehouse-docs.toml; default: <project>/lakehouse-docs.toml")
    g.add_argument("--strict", action="store_true", help="exit 1 when lineage has unresolved columns")

    c = sub.add_parser("collect", help="save ekos-lakehouse --json outputs and dbt artifacts")
    common(c)
    c.add_argument("--binary", default="ekos-lakehouse")
    c.add_argument("--check", action="store_true", help="also run `lakehouse check --lint`")
    c.add_argument("--reconcile", action="store_true", help="also run `lakehouse reconcile` (queries source + Trino)")
    c.add_argument("--table", action="append", default=[], help="limit --reconcile to these tables")

    a = sub.add_parser("all", help="collect, then generate")
    common(a)
    a.add_argument("--config", type=Path)
    a.add_argument("--binary", default="ekos-lakehouse")
    a.add_argument("--check", action="store_true")
    a.add_argument("--reconcile", action="store_true")
    a.add_argument("--table", action="append", default=[])
    a.add_argument("--strict", action="store_true")

    args = ap.parse_args(argv)
    project = args.project.resolve()
    pipeline = (args.pipeline or project / "pipeline").resolve()
    out = (args.out or pipeline / "docs").resolve()
    evidence = (args.evidence or out / "_evidence").resolve()

    if args.cmd in ("collect", "all"):
        meta = collect(project, pipeline, evidence, args.binary, args.check, args.reconcile, args.table)
        for k, v in meta["report"].items():
            print(f"collect {k:<14} {v}")
    if args.cmd in ("generate", "all"):
        cfg = args.config or (project / "lakehouse-docs.toml")
        res = generate(pipeline, out, evidence, cfg if cfg.exists() else None)
        print(f"wrote {len(res.written)} page(s), {len(res.unchanged)} unchanged, {len(res.removed)} removed → {out}")
        for w in res.warnings:
            print(f"warning: {w}", file=sys.stderr)
        if args.strict and any(w.startswith("lineage:") for w in res.warnings):
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

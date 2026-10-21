"""Write the documentation set to disk.

* Manual sections already on disk are carried into the regenerated pages.
* A generated page (front matter ``ekos_generated: true``) that is no longer produced — a dropped
  table, a removed mart — is deleted. Pages people add by hand (no such marker) are never touched.
* Files are rewritten only when their content changed, so the publishers and git see real diffs.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from . import config, lineage, project
from .md import keep_manual
from .pages import Site


@dataclass
class Result:
    written: list[str]
    unchanged: list[str]
    removed: list[str]
    warnings: list[str]


def generate(pipeline: Path, out: Path, evidence: Path | None, cfg_path: Path | None) -> Result:
    cfg = config.load(cfg_path)
    p = project.load(pipeline, evidence)
    lin = lineage.build(p)
    site = Site(p, cfg, lin)
    pages = site.build()

    out.mkdir(parents=True, exist_ok=True)
    produced: set[Path] = set()
    res = Result([], [], [], [])
    for page in pages:
        target = out / page.path
        target.parent.mkdir(parents=True, exist_ok=True)
        text = keep_manual(page.render(), target)
        produced.add(target.resolve())
        if target.exists() and target.read_text(encoding="utf-8") == text:
            res.unchanged.append(page.path)
            continue
        target.write_text(text, encoding="utf-8")
        res.written.append(page.path)

    for old in out.rglob("*.md"):
        if old.resolve() in produced or "_evidence" in old.parts:
            continue
        head = old.read_text(encoding="utf-8")[:400]
        if "ekos_generated: true" in head:
            old.unlink()
            res.removed.append(str(old.relative_to(out)))
    for d in sorted((x for x in out.rglob("*") if x.is_dir()), key=lambda x: -len(x.parts)):
        if not any(d.iterdir()):
            d.rmdir()

    res.warnings += [f"lineage: {u}" for u in lin.unresolved]
    if not any([p.evidence.status, p.evidence.check, p.evidence.reconcile, p.evidence.run_results]):
        res.warnings.append("no run evidence found: quality pages show definitions only (run `collect` first)")
    return res

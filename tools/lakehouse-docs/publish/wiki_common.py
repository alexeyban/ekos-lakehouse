"""Shared by the three publishers: read a folder of Markdown pages into a tree, rewrite links,
find Mermaid diagrams and images, render diagrams to images when a target cannot draw them.

Conventions (the generator follows them; hand-written docs can too):

* ``<dir>/index.md`` is the page for ``<dir>``; every other ``.md`` in ``<dir>`` is its child.
  A folder without ``index.md`` adds no level: its pages hang under the nearest ancestor page.
* Front matter ``title:`` names the page (else the first ``# heading``, else the file name);
  ``order:`` sorts siblings. The leading ``# heading`` is removed, since every wiki shows the title.
* Links to other ``.md`` files are relative; ``#anchor`` suffixes are kept where the target supports them.
* Files and folders starting with ``_`` (e.g. ``_evidence``) and ``.`` are not published.

Standard library + PyYAML only, so every publisher stays a single static script.
"""

from __future__ import annotations

import argparse
import hashlib
import logging
import os
import posixpath
import re
import shutil
import subprocess
import tempfile
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

import yaml

log = logging.getLogger("ekos-docs")

MANAGED = "ekos-docs:managed"
FRONT = re.compile(r"^---\s*\n(.*?)\n---\s*\n?", re.S)
MERMAID = re.compile(r"^```mermaid[ \t]*\n(.*?)^```[ \t]*$", re.S | re.M)
FENCE = re.compile(r"^(```|~~~)([^\n]*)\n(.*?)^\1[ \t]*$", re.S | re.M)
LINK = re.compile(r"(!?)\[((?:[^\[\]]|\[[^\]]*\])*)\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
MANUAL_MARK = re.compile(r"<!-- /?ekos:manual[^>]*-->\n?")
IMAGE_EXT = {".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp"}


@dataclass
class Page:
    path: str                     # posix path relative to the docs root, e.g. "source/index.md"
    title: str
    order: int
    body: str                     # Markdown without front matter and leading H1
    parent: str | None = None
    children: list["Page"] = field(default_factory=list)
    synthetic: bool = False

    @property
    def digest(self) -> str:
        return hashlib.sha256((self.title + "\n" + self.body).encode()).hexdigest()[:16]

    def walk(self):
        yield self
        for c in sorted(self.children, key=lambda p: (p.order, p.title.lower())):
            yield from c.walk()

    def ancestors(self, by_path: dict[str, "Page"]) -> list["Page"]:
        out, cur = [], self
        while cur.parent:
            cur = by_path[cur.parent]
            out.append(cur)
        return list(reversed(out))


@dataclass
class Tree:
    root: Page
    pages: dict[str, Page]
    docs_dir: Path

    def ordered(self) -> list[Page]:
        """Parents before children, siblings by ``order``."""
        return list(self.root.walk())


def _parse(path: Path, rel: str) -> Page:
    text = path.read_text(encoding="utf-8")
    meta: dict = {}
    m = FRONT.match(text)
    if m:
        try:
            meta = yaml.safe_load(m.group(1)) or {}
        except yaml.YAMLError:
            meta = {}
        text = text[m.end():]
    body = text.lstrip("\n")
    h1 = re.match(r"#\s+(.+)\n", body)
    title = str(meta.get("title") or (h1.group(1).strip() if h1 else Path(rel).stem.replace("-", " ").replace("_", " ").title()))
    if h1:
        body = body[h1.end():].lstrip("\n")
    return Page(rel, title, int(meta.get("order", 1000)), body)


def load_tree(docs: Path, root_title: str | None = None) -> Tree:
    docs = docs.resolve()
    pages: dict[str, Page] = {}
    for f in sorted(docs.rglob("*.md")):
        rel = f.relative_to(docs).as_posix()
        if any(part.startswith(("_", ".")) for part in Path(rel).parts):
            continue
        pages[rel] = _parse(f, rel)
    if "index.md" not in pages:
        pages["index.md"] = Page("index.md", root_title or docs.name, 0, "", synthetic=True)
    if root_title:
        pages["index.md"].title = root_title
    # A folder without index.md adds no level: its pages belong to the nearest ancestor page.
    for rel, page in pages.items():
        if rel == "index.md":
            continue
        d = posixpath.dirname(rel)
        if posixpath.basename(rel) == "index.md":
            d = posixpath.dirname(d)
        while d and f"{d}/index.md" not in pages:
            d = posixpath.dirname(d)
        page.parent = f"{d}/index.md" if d else "index.md"
        pages[page.parent].children.append(page)
    root = pages["index.md"]
    if root.synthetic:
        root.body = "\n".join(f"- [{c.title}]({c.path})" for c in sorted(root.children, key=lambda p: (p.order, p.title))) + "\n"
    return Tree(pages["index.md"], pages, docs)


# ── body transforms ────────────────────────────────────────────────────────────────────────────


def strip_manual_markers(body: str) -> str:
    return MANUAL_MARK.sub("", body)


def resolve(page: Page, target: str) -> tuple[str | None, str]:
    """A link target relative to ``page`` → (path under the docs root or None if external, anchor)."""
    if re.match(r"^[a-z][a-z0-9+.-]*:", target, re.I) or target.startswith("//"):
        return None, ""
    path, _, anchor = target.partition("#")
    if not path:
        return page.path, anchor
    joined = posixpath.normpath(posixpath.join(posixpath.dirname(page.path), path))
    return joined, anchor


def rewrite_links(page: Page, body: str,
                  on_page: Callable[[str, str, str], str | None],
                  on_asset: Callable[[str, str, bool], str | None]) -> str:
    """Rewrite every Markdown link/image outside code. ``on_page(path, anchor, text)`` returns the
    replacement link markup (or None to keep it); ``on_asset(path, text, is_image)`` likewise."""

    def repl(m: re.Match[str]) -> str:
        bang, text, target = m.groups()
        path, anchor = resolve(page, target)
        if path is None:
            return m.group(0)
        if path.endswith(".md") and not bang:
            out = on_page(path, anchor, text)
        else:
            out = on_asset(path, text, bool(bang))
        return out if out is not None else m.group(0)

    return map_outside_code(body, lambda chunk: LINK.sub(repl, chunk))


def map_outside_code(body: str, fn: Callable[[str], str]) -> str:
    """Apply ``fn`` to the text between fenced code blocks and inline code spans."""
    out, pos = [], 0
    for m in FENCE.finditer(body):
        out.append(_map_inline(body[pos:m.start()], fn))
        out.append(m.group(0))
        pos = m.end()
    out.append(_map_inline(body[pos:], fn))
    return "".join(out)


def _map_inline(text: str, fn: Callable[[str], str]) -> str:
    parts = re.split(r"(`[^`\n]*`)", text)
    return "".join(p if p.startswith("`") and p.endswith("`") and len(p) > 1 else fn(p) for p in parts)


def mermaid_blocks(body: str) -> list[str]:
    return [m.group(1) for m in MERMAID.finditer(body)]


def replace_mermaid(body: str, fn: Callable[[str, int], str]) -> str:
    counter = iter(range(10_000))
    return MERMAID.sub(lambda m: fn(m.group(1), next(counter)), body)


# ── Mermaid rendering ──────────────────────────────────────────────────────────────────────────


def mmdc_available() -> str | None:
    return os.environ.get("MMDC") or shutil.which("mmdc")


def render_mermaid(source: str, fmt: str = "png", cache: Path | None = None) -> tuple[str, bytes] | None:
    """Render one diagram with mermaid-cli (``npm i -g @mermaid-js/mermaid-cli``). Returns
    (file name, bytes); the name is content-addressed, so an unchanged diagram is not re-uploaded."""
    exe = mmdc_available()
    if not exe:
        return None
    digest = hashlib.sha256(source.encode()).hexdigest()[:12]
    name = f"mermaid-{digest}.{fmt}"
    if cache is not None and (cache / name).exists():
        return name, (cache / name).read_bytes()
    with tempfile.TemporaryDirectory() as d:
        src, dst = Path(d) / "d.mmd", Path(d) / name
        src.write_text(source, encoding="utf-8")
        args = [exe, "-i", str(src), "-o", str(dst), "-b", "white", "-q"]
        if fmt == "png":
            args += ["-s", "2"]
        cfg = os.environ.get("MMDC_PUPPETEER_CONFIG")
        if cfg:
            args += ["-p", cfg]
        r = subprocess.run(args, capture_output=True, text=True)
        if r.returncode != 0 or not dst.exists():
            log.warning("mermaid render failed: %s", (r.stderr or "")[-300:])
            return None
        data = dst.read_bytes()
    if cache is not None:
        cache.mkdir(parents=True, exist_ok=True)
        (cache / name).write_bytes(data)
    return name, data


# ── CLI helpers ────────────────────────────────────────────────────────────────────────────────


def base_args(description: str) -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(description=description)
    ap.add_argument("--docs", type=Path, required=True, help="folder of .md pages (e.g. pipeline/docs)")
    ap.add_argument("--root-title", help="title of the top page (default: the docs' own index title)")
    ap.add_argument("--dry-run", action="store_true", help="print what would change; send nothing")
    ap.add_argument("--prune", action="store_true", help="delete previously published pages that no longer exist locally (only pages this tool created)")
    ap.add_argument("-v", "--verbose", action="store_true")
    return ap


def setup_logging(verbose: bool) -> None:
    logging.basicConfig(level=logging.DEBUG if verbose else logging.INFO, format="%(message)s")


def env(name: str, required: bool = True, default: str | None = None) -> str | None:
    v = os.environ.get(name, default)
    if required and not v:
        raise SystemExit(f"environment variable {name} is required")
    return v

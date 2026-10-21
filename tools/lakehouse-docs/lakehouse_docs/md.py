"""Markdown helpers: tables, escaping, front matter, manual sections and Mermaid diagrams."""

from __future__ import annotations

import re
from collections.abc import Iterable, Sequence
from pathlib import Path

MANUAL_RE = re.compile(
    r"<!-- ekos:manual id=\"(?P<id>[\w.-]+)\" -->\n(?P<body>.*?)\n?<!-- /ekos:manual -->", re.S
)


def esc(v: object) -> str:
    """Escape a value for a Markdown table cell."""
    if v is None:
        return ""
    s = str(v).replace("\r", "")
    s = s.replace("|", "\\|").replace("\n", "<br>")
    return s


def code(v: object) -> str:
    if v is None or v == "":
        return ""
    s = str(v).replace("`", "'")
    return f"`{esc(s)}`"


def table(headers: Sequence[str], rows: Iterable[Sequence[object]]) -> str:
    rows = list(rows)
    if not rows:
        return "_None._\n"
    out = ["| " + " | ".join(headers) + " |", "|" + "|".join("---" for _ in headers) + "|"]
    for r in rows:
        out.append("| " + " | ".join(esc(c) if not str(c).startswith("`") else str(c) for c in r) + " |")
    return "\n".join(out) + "\n"


def front_matter(title: str, order: int, **extra: object) -> str:
    lines = ["---", f"title: {_yaml_str(title)}", f"order: {order}", "ekos_generated: true"]
    for k, v in extra.items():
        if v is not None:
            lines.append(f"{k}: {_yaml_str(str(v))}")
    lines.append("---\n")
    return "\n".join(lines)


def _yaml_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def manual(id: str, default: str) -> str:
    """A section people edit. Regeneration keeps whatever is between the markers."""
    return f'<!-- ekos:manual id="{id}" -->\n{default.strip()}\n<!-- /ekos:manual -->\n'


def keep_manual(new: str, old_path: Path) -> str:
    """Carry each manual section's body from the page already on disk into the new page."""
    if not old_path.exists():
        return new
    old = {m.group("id"): m.group("body") for m in MANUAL_RE.finditer(old_path.read_text(encoding="utf-8"))}
    if not old:
        return new

    def repl(m: re.Match[str]) -> str:
        body = old.get(m.group("id"), m.group("body"))
        return f'<!-- ekos:manual id="{m.group("id")}" -->\n{body}\n<!-- /ekos:manual -->'

    return MANUAL_RE.sub(repl, new)


def slug(s: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "-", s).strip("-").lower()


def rel_link(from_page: str, to_page: str) -> str:
    """A relative link between two pages given as paths under the docs root."""
    a = Path(from_page).parent
    b = Path(to_page)
    try:
        return str(Path(*([".."] * len(a.parts)), b)) if a.parts else str(b)
    except TypeError:
        return str(b)


# ── Mermaid ────────────────────────────────────────────────────────────────────────────────────


def mermaid(body: str) -> str:
    return f"```mermaid\n{body.rstrip()}\n```\n"


def m_id(s: str) -> str:
    """An identifier Mermaid accepts for an entity or node."""
    return re.sub(r"[^A-Za-z0-9_]", "_", s)


def m_type(t: str) -> str:
    """An erDiagram attribute type: one word, no punctuation."""
    t = re.sub(r"\(.*?\)", "", t or "unknown").strip()
    t = re.sub(r"[^A-Za-z0-9_]+", "_", t).strip("_")
    return t or "unknown"


def m_label(s: str) -> str:
    return s.replace('"', "'").replace("\n", " ")[:80]

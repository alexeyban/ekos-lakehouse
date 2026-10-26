"""The book as one PDF, printed by Chromium from one HTML page of every page in order."""

from __future__ import annotations

import re
from pathlib import Path

import markdown

FRONT = re.compile(r"^---\n.*?\n---\n", re.S)
MANUAL = re.compile(r"<!-- /?ekos:manual[^>]*-->\n?")
STYLE = """
body { font: 11pt/1.45 system-ui, sans-serif; margin: 0 2cm; color: #1d2530; }
h1 { page-break-before: always; font-size: 20pt; } h1:first-of-type { page-break-before: avoid; }
img { max-width: 100%; border: 1px solid #ccd; } pre { white-space: pre-wrap; font-size: 7.5pt;
background: #f4f6f8; padding: 6px; } table { border-collapse: collapse; } td, th { border: 1px solid #ccd;
padding: 2px 6px; font-size: 9pt; } a { color: #1a5fb4; }
"""


def html_of(book: Path, pages: list[str]) -> str:
    parts = []
    for name in pages:
        text = MANUAL.sub("", FRONT.sub("", (book / name).read_text(encoding="utf-8"), count=1))
        text = re.sub(r"\(step-(\d)\.md\)", r"(#step-\1)", text)
        parts.append(f'<a id="{Path(name).stem}"></a>\n' + markdown.markdown(text, extensions=["tables", "fenced_code"]))
    return f"<!doctype html><meta charset=utf-8><style>{STYLE}</style>" + "\n".join(parts)


def write_pdf(book: Path, pages: list[str], out: Path) -> None:
    from playwright.sync_api import sync_playwright

    html = book / "_book.html"
    html.write_text(html_of(book, pages), encoding="utf-8")
    with sync_playwright() as p:
        b = p.chromium.launch(args=["--no-sandbox"])
        page = b.new_page()
        page.goto(html.resolve().as_uri())
        page.pdf(path=str(out), format="A4", print_background=True,
                 margin={"top": "1.5cm", "bottom": "1.5cm", "left": "0", "right": "0"})
        b.close()
    html.unlink()

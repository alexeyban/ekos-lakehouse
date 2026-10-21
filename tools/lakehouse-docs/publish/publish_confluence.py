#!/usr/bin/env python3
"""Publish a folder of Markdown pages to Confluence (Cloud or Data Center/Server).

    export CONFLUENCE_URL=https://acme.atlassian.net/wiki     # DC: https://confluence.acme.com
    export CONFLUENCE_USER=me@acme.com CONFLUENCE_API_TOKEN=…  # Cloud: email + API token
    # or: export CONFLUENCE_PAT=…                              # DC/Server: personal access token
    python publish_confluence.py --docs pipeline/docs --space DATA --parent-id 123456

What it does
* Mirrors the folder hierarchy as a page tree under ``--parent-id`` (or at the space root).
* Converts Markdown to Confluence storage format: tables, code blocks → code macro, page links →
  real Confluence page links, local images → attachments.
* Mermaid: ``--mermaid image`` (default) renders each diagram to PNG with mermaid-cli and attaches
  it; ``--mermaid macro --mermaid-macro <name>`` hands the source to a Mermaid app's macro;
  ``--mermaid code`` keeps the source as a code block.
* Idempotent: each page's content hash is stored in its version message; an unchanged page is
  not updated, so page history only shows real changes. Pages are labelled ``ekos-docs``;
  ``--prune`` deletes labelled pages under the root that no longer exist locally.

Requires: requests, Markdown, PyYAML (and mermaid-cli ``mmdc`` for --mermaid image).
"""

from __future__ import annotations

import hashlib
import html
import json
import mimetypes
import posixpath
import re
import sys
from pathlib import Path

import markdown
import requests

sys.path.insert(0, str(Path(__file__).resolve().parent))
from wiki_common import (  # noqa: E402
    FENCE, Page, Tree, base_args, env, load_tree, log, render_mermaid,
    replace_mermaid, resolve, setup_logging, strip_manual_markers,
)

LABEL = "ekos-docs"
LANGS = {"sql": "sql", "bash": "bash", "sh": "bash", "shell": "bash", "yaml": "yaml", "yml": "yaml",
         "json": "javascript", "python": "python", "py": "python", "toml": "text", "": "text"}


def cdata(s: str) -> str:
    return "<![CDATA[" + s.replace("]]>", "]]]]><![CDATA[>") + "]]>"


class Confluence:
    def __init__(self, base: str, space: str, dry_run: bool) -> None:
        self.base = base.rstrip("/")
        self.space = space
        self.dry = dry_run
        self.s = requests.Session()
        pat = env("CONFLUENCE_PAT", required=False)
        if pat:
            self.s.headers["Authorization"] = f"Bearer {pat}"
        else:
            self.s.auth = (env("CONFLUENCE_USER"), env("CONFLUENCE_API_TOKEN"))
        self.s.headers["Accept"] = "application/json"

    def _req(self, method: str, path: str, **kw) -> requests.Response:
        r = self.s.request(method, f"{self.base}/rest/api{path}", timeout=60, **kw)
        if r.status_code >= 400:
            raise RuntimeError(f"{method} {path} → {r.status_code}: {r.text[:500]}")
        return r

    def find(self, title: str) -> dict | None:
        r = self._req("GET", "/content", params={"spaceKey": self.space, "title": title, "type": "page",
                                                 "expand": "version,ancestors,metadata.labels"})
        res = r.json().get("results", [])
        return res[0] if res else None

    def create(self, title: str, parent_id: str | None, body: str, message: str) -> dict:
        data = {"type": "page", "title": title, "space": {"key": self.space},
                "body": {"storage": {"value": body, "representation": "storage"}},
                "version": {"message": message}}
        if parent_id:
            data["ancestors"] = [{"id": parent_id}]
        page = self._req("POST", "/content", json=data).json()
        self._req("POST", f"/content/{page['id']}/label", json=[{"prefix": "global", "name": LABEL}])
        return page

    def update(self, page: dict, title: str, parent_id: str | None, body: str, message: str) -> dict:
        data = {"id": page["id"], "type": "page", "title": title, "space": {"key": self.space},
                "body": {"storage": {"value": body, "representation": "storage"}},
                "version": {"number": page["version"]["number"] + 1, "message": message}}
        if parent_id:
            data["ancestors"] = [{"id": parent_id}]
        return self._req("PUT", f"/content/{page['id']}", json=data).json()

    def attach(self, page_id: str, name: str, data: bytes) -> None:
        existing = self._req("GET", f"/content/{page_id}/child/attachment", params={"filename": name}).json().get("results", [])
        mime = mimetypes.guess_type(name)[0] or "application/octet-stream"
        files = {"file": (name, data, mime)}
        headers = {"X-Atlassian-Token": "nocheck"}
        if existing:
            # Content-addressed names: an existing attachment already holds these bytes.
            if not name.startswith("mermaid-"):
                self._req("POST", f"/content/{page_id}/child/attachment/{existing[0]['id']}/data", files=files, headers=headers)
            return
        self._req("POST", f"/content/{page_id}/child/attachment", files=files, headers=headers, data={"minorEdit": "true"})

    def labelled_under(self, root_id: str) -> list[dict]:
        out, start = [], 0
        while True:
            r = self._req("GET", "/content/search", params={"cql": f'label = "{LABEL}" and ancestor = {root_id}', "limit": 100, "start": start})
            res = r.json().get("results", [])
            out += res
            if len(res) < 100:
                return out
            start += 100

    def delete(self, page_id: str) -> None:
        self._req("DELETE", f"/content/{page_id}")


def to_storage(tree: Tree, page: Page, titles: dict[str, str], mermaid_mode: str, macro: str,
               attachments: dict[str, bytes], cache: Path) -> str:
    blocks: dict[str, str] = {}

    def token(xml: str) -> str:
        t = f"EKOSBLOCK{len(blocks):05d}X"
        blocks[t] = xml
        return f"\n\n{t}\n\n"

    body = strip_manual_markers(page.body)

    def mermaid(src: str, _i: int) -> str:
        if mermaid_mode == "image":
            r = render_mermaid(src, "png", cache)
            if r:
                name, data = r
                attachments[name] = data
                return token(f'<ac:image ac:align="center"><ri:attachment ri:filename="{name}" /></ac:image>')
            log.warning("  mmdc unavailable or failed — diagram kept as code on %s", page.path)
        if mermaid_mode == "macro":
            return token(f'<ac:structured-macro ac:name="{html.escape(macro)}"><ac:plain-text-body>{cdata(src)}</ac:plain-text-body></ac:structured-macro>')
        return token(f'<ac:structured-macro ac:name="code"><ac:parameter ac:name="language">text</ac:parameter><ac:parameter ac:name="title">Mermaid</ac:parameter><ac:plain-text-body>{cdata(src)}</ac:plain-text-body></ac:structured-macro>')

    body = replace_mermaid(body, mermaid)

    def fence(m: re.Match[str]) -> str:
        lang = LANGS.get(m.group(2).strip().lower(), "text")
        return token(f'<ac:structured-macro ac:name="code"><ac:parameter ac:name="language">{lang}</ac:parameter><ac:plain-text-body>{cdata(m.group(3).rstrip(chr(10)))}</ac:plain-text-body></ac:structured-macro>')

    body = FENCE.sub(fence, body)
    xhtml = markdown.markdown(body, extensions=["tables", "sane_lists"], output_format="xhtml")

    def link(m: re.Match[str]) -> str:
        href, inner = html.unescape(m.group(1)), m.group(2)
        path, anchor = resolve(page, href)
        if path is None:
            return m.group(0)
        if path.endswith(".md"):
            if path not in titles:
                log.warning("  broken link on %s → %s", page.path, href)
                return inner
            a = f' ac:anchor="{html.escape(anchor)}"' if anchor and path == page.path else ""
            return f'<ac:link{a}><ri:page ri:content-title="{html.escape(titles[path])}" /><ac:link-body>{inner}</ac:link-body></ac:link>'
        f = tree.docs_dir / path
        if f.exists():
            attachments[posixpath.basename(path)] = f.read_bytes()
            return f'<ac:link><ri:attachment ri:filename="{html.escape(posixpath.basename(path))}" /><ac:link-body>{inner}</ac:link-body></ac:link>'
        return inner

    def img(m: re.Match[str]) -> str:
        src = html.unescape(m.group(2))
        path, _ = resolve(page, src)
        if path is None:
            return f'<ac:image><ri:url ri:value="{html.escape(src)}" /></ac:image>'
        f = tree.docs_dir / path
        if not f.exists():
            return m.group(0)
        name = posixpath.basename(path)
        attachments[name] = f.read_bytes()
        return f'<ac:image><ri:attachment ri:filename="{html.escape(name)}" /></ac:image>'

    xhtml = re.sub(r'<a href="([^"]*)">(.*?)</a>', link, xhtml, flags=re.S)
    xhtml = re.sub(r'<img alt="([^"]*)" src="([^"]*)"\s*/?>', img, xhtml)
    xhtml = re.sub(r"<br\s*>", "<br />", xhtml)
    for t, xml in blocks.items():
        xhtml = xhtml.replace(f"<p>{t}</p>", xml).replace(t, xml)
    banner = ('<ac:structured-macro ac:name="info"><ac:rich-text-body><p>Generated from the pipeline repository '
              'by ekos-lakehouse docs. Edit the Markdown in Git — changes made here are overwritten on the next publish.'
              '</p></ac:rich-text-body></ac:structured-macro>')
    return banner + xhtml


def main() -> int:
    ap = base_args("Publish Markdown docs to Confluence.")
    ap.add_argument("--space", default=env("CONFLUENCE_SPACE", required=False), help="space key (or CONFLUENCE_SPACE)")
    ap.add_argument("--parent-id", default=env("CONFLUENCE_PARENT_ID", required=False), help="page id to publish under (default: space root)")
    ap.add_argument("--title-prefix", default="", help="prefix for every title — Confluence titles are unique per space")
    ap.add_argument("--mermaid", choices=["image", "macro", "code"], default="image")
    ap.add_argument("--mermaid-macro", default="mermaid-cloud", help="macro name of your Mermaid app (with --mermaid macro)")
    ap.add_argument("--adopt", action="store_true", help="allow updating same-titled pages this tool did not create")
    args = ap.parse_args()
    setup_logging(args.verbose)
    if not args.space:
        raise SystemExit("--space or CONFLUENCE_SPACE is required")
    tree = load_tree(args.docs, args.root_title)
    titles = {p.path: f"{args.title_prefix}{p.title}" for p in tree.ordered()}
    dupes = {t for t in titles.values() if list(titles.values()).count(t) > 1}
    if dupes:
        raise SystemExit(f"duplicate page titles (Confluence needs unique titles per space): {sorted(dupes)} — use --title-prefix or rename")
    cf = Confluence(env("CONFLUENCE_URL"), args.space, args.dry_run)
    cache = args.docs / "_evidence" / ".mermaid-cache"
    ids: dict[str, str] = {}
    created = updated = same = 0
    for page in tree.ordered():
        title = titles[page.path]
        parent_id = ids.get(page.parent) if page.parent else args.parent_id
        attachments: dict[str, bytes] = {}
        storage = to_storage(tree, page, titles, args.mermaid, args.mermaid_macro, attachments, cache)
        digest = hashlib.sha256(json.dumps([title, parent_id or "", storage, sorted(attachments)]).encode()).hexdigest()[:16]
        message = f"ekos-docs {digest}"
        existing = cf.find(title)
        if existing and not args.adopt:
            labels = {x.get("name") for x in existing.get("metadata", {}).get("labels", {}).get("results", [])}
            if LABEL not in labels:
                raise SystemExit(
                    f"a page titled {title!r} already exists in space {args.space} and was not published by this tool — "
                    "refusing to overwrite it. Use --title-prefix, or --adopt to take it over."
                )
        if args.dry_run:
            state = "create" if not existing else "unchanged" if existing["version"].get("message") == message else "update"
            log.info("%-9s %s  (%d attachment(s))", state, title, len(attachments))
            ids[page.path] = existing["id"] if existing else f"new:{page.path}"
            continue
        if existing and existing["version"].get("message") == message:
            ids[page.path] = existing["id"]
            same += 1
            log.debug("unchanged %s", title)
            continue
        if existing and LABEL not in {x.get("name") for x in existing.get("metadata", {}).get("labels", {}).get("results", [])}:
            cf._req("POST", f"/content/{existing['id']}/label", json=[{"prefix": "global", "name": LABEL}])
        if not existing:
            # Create first (attachments need a page), then attach, then write the real body.
            existing = cf.create(title, parent_id, "<p>…</p>", "ekos-docs create")
            existing.setdefault("version", {"number": 1})
            created += 1
            log.info("created   %s", title)
        else:
            updated += 1
            log.info("updated   %s", title)
        for name, data in attachments.items():
            cf.attach(existing["id"], name, data)
        existing = cf.find(title) or existing
        cf.update(existing, title, parent_id, storage, message)
        ids[page.path] = existing["id"]
    log.info("confluence: %d created, %d updated, %d unchanged", created, updated, same)
    if args.prune and not args.dry_run:
        root_id = ids.get("index.md")
        keep = set(ids.values())
        for p in cf.labelled_under(root_id):
            if p["id"] not in keep:
                cf.delete(p["id"])
                log.info("deleted   %s", p.get("title"))
    return 0


if __name__ == "__main__":
    sys.exit(main())

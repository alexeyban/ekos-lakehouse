#!/usr/bin/env python3
"""Publish a folder of Markdown pages to an Azure DevOps wiki (project wiki or code wiki).

    export AZDO_ORG_URL=https://dev.azure.com/acme  AZDO_PROJECT=Data  AZDO_PAT=…
    python publish_azure_devops_wiki.py --docs pipeline/docs --wiki Data.wiki --root-path "/Lakehouse/Pagila"

The PAT needs **Wiki (Read & Write)**. For a code wiki (published from a repo branch) pass
``--branch main``; for the default project wiki leave it out.

What it does
* Mirrors the folder hierarchy as pages under ``--root-path`` (page names are the titles).
* Mermaid: ``--mermaid native`` (default) converts ```` ```mermaid ```` fences to Azure DevOps'
  ``::: mermaid`` blocks, which the wiki renders; ``--mermaid image`` renders PNGs with
  mermaid-cli and uploads them as wiki attachments.
* Rewrites links between pages to wiki paths; uploads local images to ``/.attachments``.
* Idempotent: a hidden marker with the content hash ends each page; unchanged pages are skipped.
  ``--prune`` deletes pages under the root that carry the marker and no longer exist locally.

Requires: requests, PyYAML (and mermaid-cli ``mmdc`` for --mermaid image).
"""

from __future__ import annotations

import base64
import hashlib
import re
import sys
from pathlib import Path
from urllib.parse import quote

import requests

sys.path.insert(0, str(Path(__file__).resolve().parent))
from wiki_common import (  # noqa: E402
    MANAGED, Page, Tree, base_args, env, load_tree, log, render_mermaid, replace_mermaid,
    rewrite_links, setup_logging, strip_manual_markers,
)

API = "7.1"
MARK = re.compile(r"\n?<!-- " + re.escape(MANAGED) + r" hash=(\w+) -->\s*$")
# Characters a wiki page name cannot contain.
BAD = re.compile(r'[\\/#?:<>*|"%]')


def page_name(title: str) -> str:
    return re.sub(r"\s+", " ", BAD.sub(" ", title)).strip() or "Untitled"


def link_path(path: str) -> str:
    """A wiki path as a Markdown link target: '-' → %2D, then spaces → '-'."""
    segs = []
    for seg in path.strip("/").split("/"):
        seg = seg.replace("-", "%2D")
        segs.append(quote(seg, safe="%() ").replace(" ", "-"))
    return "/" + "/".join(segs)


class AzureWiki:
    def __init__(self, org: str, project: str, wiki: str, branch: str | None) -> None:
        self.base = f"{org.rstrip('/')}/{quote(project)}/_apis/wiki/wikis/{quote(wiki)}"
        self.branch = branch
        self.s = requests.Session()
        self.s.auth = ("", env("AZDO_PAT"))

    def _params(self, **kw) -> dict:
        p = {"api-version": API, **kw}
        if self.branch:
            p["versionDescriptor.version"] = self.branch
            p["versionDescriptor.versionType"] = "branch"
        return p

    def get(self, path: str, recursion: str | None = None) -> tuple[dict | None, str | None]:
        params = self._params(path=path, includeContent="true")
        if recursion:
            params["recursionLevel"] = recursion
            params["includeContent"] = "false"
        r = self.s.get(f"{self.base}/pages", params=params, timeout=60)
        if r.status_code == 404:
            return None, None
        if r.status_code >= 400:
            raise RuntimeError(f"GET {path} → {r.status_code}: {r.text[:400]}")
        return r.json(), r.headers.get("ETag")

    def put(self, path: str, content: str, etag: str | None, comment: str) -> None:
        headers = {"Content-Type": "application/json"}
        if etag:
            headers["If-Match"] = etag
        r = self.s.put(f"{self.base}/pages", params=self._params(path=path, comment=comment),
                       json={"content": content}, headers=headers, timeout=60)
        if r.status_code >= 400:
            raise RuntimeError(f"PUT {path} → {r.status_code}: {r.text[:400]}")

    def delete(self, path: str) -> None:
        r = self.s.delete(f"{self.base}/pages", params=self._params(path=path, comment="ekos-docs prune"), timeout=60)
        if r.status_code >= 400 and r.status_code != 404:
            raise RuntimeError(f"DELETE {path} → {r.status_code}: {r.text[:400]}")

    def attach(self, name: str, data: bytes) -> str:
        r = self.s.put(f"{self.base}/attachments", params=self._params(name=name),
                       data=base64.b64encode(data), headers={"Content-Type": "application/octet-stream"}, timeout=120)
        if r.status_code == 409 or (r.status_code >= 400 and "already exists" in r.text.lower()):
            return f"/.attachments/{name}"     # content-addressed name: same bytes already there
        if r.status_code >= 400:
            raise RuntimeError(f"attachment {name} → {r.status_code}: {r.text[:400]}")
        return r.json().get("path") or f"/.attachments/{name}"


def build_paths(tree: Tree, root_path: str) -> dict[str, str]:
    paths: dict[str, str] = {}
    root = "/" + root_path.strip("/") if root_path.strip("/") else ""
    for p in tree.ordered():
        if p.path == "index.md":
            paths[p.path] = root or "/" + page_name(p.title)
        else:
            paths[p.path] = f"{paths[p.parent]}/{page_name(p.title)}"
    seen: dict[str, str] = {}
    for k, v in paths.items():
        if v.lower() in seen:
            raise SystemExit(f"two pages map to the same wiki path {v!r}: {seen[v.lower()]} and {k}")
        seen[v.lower()] = k
    return paths


def to_wiki(tree: Tree, page: Page, paths: dict[str, str], mode: str, uploads: dict[str, bytes], cache: Path) -> str:
    body = strip_manual_markers(page.body)

    def mermaid(src: str, _i: int) -> str:
        if mode == "image":
            r = render_mermaid(src, "png", cache)
            if r:
                uploads[r[0]] = r[1]
                return f"![diagram](/.attachments/{r[0]})\n"
        return f"::: mermaid\n{src.rstrip()}\n:::\n"

    body = replace_mermaid(body, mermaid)

    def on_page(path: str, anchor: str, text: str) -> str | None:
        if path not in paths:
            log.warning("  broken link on %s → %s", page.path, path)
            return text
        return f"[{text}]({link_path(paths[path])}{'#' + anchor if anchor else ''})"

    def on_asset(path: str, text: str, is_image: bool) -> str | None:
        f = tree.docs_dir / path
        if not f.exists():
            return None
        data = f.read_bytes()
        name = f"{Path(path).stem}-{hashlib.sha256(data).hexdigest()[:8]}{Path(path).suffix}"
        uploads[name] = data
        return f"{'!' if is_image else ''}[{text}](/.attachments/{name})"

    body = rewrite_links(page, body, on_page, on_asset)
    banner = "> Generated from the pipeline repository by ekos-lakehouse docs. Edit the Markdown in Git — edits made here are overwritten on the next publish.\n\n"
    if page.children and page.path != "index.md":
        body += "\n[[_TOSP_]]\n"
    return banner + body


def main() -> int:
    ap = base_args("Publish Markdown docs to an Azure DevOps wiki.")
    ap.add_argument("--org-url", default=env("AZDO_ORG_URL", required=False), help="https://dev.azure.com/<org> (or AZDO_ORG_URL)")
    ap.add_argument("--project", default=env("AZDO_PROJECT", required=False))
    ap.add_argument("--wiki", default=env("AZDO_WIKI", required=False), help="wiki name or id, e.g. <Project>.wiki")
    ap.add_argument("--branch", default=env("AZDO_WIKI_BRANCH", required=False), help="for a code wiki: the published branch")
    ap.add_argument("--root-path", default="/", help="wiki path for the top page, e.g. /Lakehouse/Pagila (parents must exist)")
    ap.add_argument("--mermaid", choices=["native", "image"], default="native")
    args = ap.parse_args()
    setup_logging(args.verbose)
    for k in ("org_url", "project", "wiki"):
        if not getattr(args, k):
            raise SystemExit(f"--{k.replace('_', '-')} is required")
    tree = load_tree(args.docs, args.root_title)
    paths = build_paths(tree, args.root_path)
    wiki = AzureWiki(args.org_url, args.project, args.wiki, args.branch)
    cache = args.docs / "_evidence" / ".mermaid-cache"
    created = updated = same = 0
    for page in tree.ordered():
        path = paths[page.path]
        uploads: dict[str, bytes] = {}
        content = to_wiki(tree, page, paths, args.mermaid, uploads, cache)
        digest = hashlib.sha256(content.encode()).hexdigest()[:16]
        content = content.rstrip() + f"\n\n<!-- {MANAGED} hash={digest} -->\n"
        existing, etag = wiki.get(path)
        old = MARK.search((existing or {}).get("content", "") or "")
        if existing and old and old.group(1) == digest:
            same += 1
            log.debug("unchanged %s", path)
            continue
        if existing and not old and (existing.get("content") or "").strip():
            raise SystemExit(f"{path} exists and was not published by this tool — refusing to overwrite it (choose another --root-path)")
        state = "update" if existing else "create"
        if args.dry_run:
            log.info("%-9s %s  (%d attachment(s))", state, path, len(uploads))
            continue
        for name, data in uploads.items():
            wiki.attach(name, data)
        wiki.put(path, content, etag if existing else None, f"ekos-docs {digest}")
        log.info("%-9s %s", state + "d", path)
        created += state == "create"
        updated += state == "update"
    log.info("azure devops wiki: %d created, %d updated, %d unchanged", created, updated, same)

    if args.prune and not args.dry_run:
        root = paths["index.md"]
        listing, _ = wiki.get(root, recursion="full")
        wanted = {p.lower() for p in paths.values()}
        stale: list[str] = []

        def walk(node: dict) -> None:
            for sub in node.get("subPages", []) or []:
                walk(sub)
            if node.get("path") and node["path"].lower() not in wanted:
                stale.append(node["path"])

        if listing:
            walk(listing)
        for path in stale:   # deepest first: walk() appends children before parents
            page, _ = wiki.get(path)
            if page and MARK.search(page.get("content", "") or ""):
                wiki.delete(path)
                log.info("deleted   %s", path)
    return 0


if __name__ == "__main__":
    sys.exit(main())

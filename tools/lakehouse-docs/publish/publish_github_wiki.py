#!/usr/bin/env python3
"""Publish a folder of Markdown pages to a GitHub wiki (the ``<repo>.wiki.git`` repository).

    export GITHUB_TOKEN=…        # a token with contents:write on the repository
    python publish_github_wiki.py --docs pipeline/docs --repo acme/pagila-lakehouse

The wiki must exist: create its first page once in the GitHub UI (GitHub creates the wiki
repository only then).

What it does
* GitHub wikis are flat: every page becomes ``<Page-Name>.md`` at the wiki root (names from the
  titles, made unique); the hierarchy is kept in a generated ``_Sidebar.md``. The top page is
  ``Home`` unless ``--prefix`` is given (several projects can then share one wiki).
* Mermaid fences are kept — GitHub renders them. Links between pages become wiki links; local
  images are copied to ``images/``.
* Idempotent: commits and pushes only when a file changed. ``--prune`` removes pages this tool
  wrote earlier (they carry a hidden marker) that no longer exist locally.

Requires: git, PyYAML.
"""

from __future__ import annotations

import base64
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from wiki_common import (  # noqa: E402
    MANAGED, Page, Tree, base_args, env, load_tree, log, rewrite_links, setup_logging,
    strip_manual_markers,
)

MARKER = f"<!-- {MANAGED} -->"


def wiki_name(title: str) -> str:
    """A GitHub wiki page name: the file name without .md; '-' shows as a space."""
    name = re.sub(r'[\\/:*?"<>|#%]', "", title)
    name = re.sub(r"\s+", "-", name.strip())
    return name or "Untitled"


def build_names(tree: Tree, prefix: str) -> dict[str, str]:
    names: dict[str, str] = {}
    used: set[str] = {"_sidebar", "_footer"}
    for p in tree.ordered():
        if p.path == "index.md":
            base = wiki_name(prefix) if prefix else "Home"
        else:
            base = wiki_name(f"{prefix} {p.title}" if prefix else p.title)
        name, i = base, 2
        while name.lower() in used:
            parent = tree.pages[p.parent].title if p.parent else ""
            name = wiki_name(f"{prefix} {parent} {p.title}" if i == 2 else f"{base} {i}")
            i += 1
        used.add(name.lower())
        names[p.path] = name
    return names


def git(cwd: Path, *args: str, auth: str | None = None) -> str:
    cmd = ["git"]
    if auth:
        cmd += ["-c", f"http.extraHeader=Authorization: Basic {auth}"]
    r = subprocess.run(cmd + list(args), cwd=cwd, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)}: {r.stderr.strip()[:500]}")
    return r.stdout


def render(tree: Tree, page: Page, names: dict[str, str], images: dict[str, Path]) -> str:
    body = strip_manual_markers(page.body)

    def on_page(path: str, anchor: str, text: str) -> str | None:
        if path not in names:
            log.warning("  broken link on %s → %s", page.path, path)
            return text
        return f"[{text}]({names[path]}{'#' + anchor if anchor else ''})"

    def on_asset(path: str, text: str, is_image: bool) -> str | None:
        f = tree.docs_dir / path
        if not f.exists():
            return None
        target = "images/" + path.replace("/", "__")
        images[target] = f
        return f"{'!' if is_image else ''}[{text}]({target})"

    body = rewrite_links(page, body, on_page, on_asset)
    return f"{MARKER}\n# {page.title}\n\n{body.rstrip()}\n"


def sidebar(tree: Tree, names: dict[str, str]) -> str:
    lines = [MARKER, f"**[{tree.root.title}]({names['index.md']})**", ""]
    for p in tree.ordered():
        if p.path == "index.md":
            continue
        depth = len(p.ancestors(tree.pages)) - 1
        lines.append(f"{'  ' * depth}- [{p.title}]({names[p.path]})")
    return "\n".join(lines) + "\n"


def main() -> int:
    ap = base_args("Publish Markdown docs to a GitHub wiki.")
    ap.add_argument("--repo", default=env("GITHUB_REPOSITORY", required=False), help="owner/name of the repository whose wiki to write")
    ap.add_argument("--remote", help="wiki git URL (default: https://<host>/<repo>.wiki.git)")
    ap.add_argument("--host", default=os.environ.get("GITHUB_SERVER_URL", "https://github.com"), help="GitHub (Enterprise) URL")
    ap.add_argument("--prefix", default="", help="page-name prefix, so several projects can share one wiki (the top page is then <prefix>, not Home)")
    ap.add_argument("--sidebar", choices=["replace", "keep"], default="replace", help="replace: write _Sidebar.md; keep: leave an existing one")
    ap.add_argument("--message", default="docs: publish from ekos-lakehouse")
    args = ap.parse_args()
    setup_logging(args.verbose)
    if not args.repo and not args.remote:
        raise SystemExit("--repo (or GITHUB_REPOSITORY) or --remote is required")
    remote = args.remote or f"{args.host.rstrip('/')}/{args.repo}.wiki.git"
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    auth = base64.b64encode(f"x-access-token:{token}".encode()).decode() if token and remote.startswith("http") else None

    tree = load_tree(args.docs, args.root_title)
    names = build_names(tree, args.prefix)
    images: dict[str, Path] = {}
    files = {f"{names[p.path]}.md": render(tree, p, names, images) for p in tree.ordered()}
    if args.sidebar == "replace":
        files["_Sidebar.md"] = sidebar(tree, names)

    work = Path(tempfile.mkdtemp(prefix="ekos-wiki-"))
    try:
        try:
            git(work, "clone", "--depth", "1", remote, "wiki", auth=auth)
        except RuntimeError as e:
            raise SystemExit(f"cannot clone {remote}: {e}\nIf the wiki is new, create its first page once in the GitHub UI.")
        repo = work / "wiki"
        changed: list[str] = []
        for name, text in files.items():
            f = repo / name
            if not f.exists() or f.read_text(encoding="utf-8") != text:
                if f.exists() and MARKER not in f.read_text(encoding="utf-8")[:200] and name != "_Sidebar.md":
                    raise SystemExit(f"{name} exists in the wiki and was not written by this tool — refusing to overwrite (use --prefix)")
                changed.append(name)
                if not args.dry_run:
                    f.write_text(text, encoding="utf-8")
        for target, src in images.items():
            f = repo / target
            if not f.exists() or f.read_bytes() != src.read_bytes():
                changed.append(target)
                if not args.dry_run:
                    f.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(src, f)
        removed: list[str] = []
        if args.prune:
            for f in repo.glob("*.md"):
                if f.name in files:
                    continue
                if MARKER in f.read_text(encoding="utf-8")[:200]:
                    if args.prefix and not f.stem.startswith(re.sub(r"\s+", "-", args.prefix)):
                        continue
                    removed.append(f.name)
                    if not args.dry_run:
                        f.unlink()
        for n in changed:
            log.info("%s %s", "would write" if args.dry_run else "write", n)
        for n in removed:
            log.info("%s %s", "would delete" if args.dry_run else "delete", n)
        if args.dry_run or not (changed or removed):
            log.info("github wiki: %d changed, %d removed%s", len(changed), len(removed), " (dry run)" if args.dry_run else "")
            return 0
        git(repo, "add", "-A")
        git(repo, "-c", "user.name=ekos-docs", "-c", "user.email=ekos-docs@users.noreply.github.com",
            "commit", "-q", "-m", args.message)
        git(repo, "push", "-q", "origin", "HEAD", auth=auth)
        log.info("github wiki: %d changed, %d removed, pushed to %s", len(changed), len(removed), remote.split("@")[-1])
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())

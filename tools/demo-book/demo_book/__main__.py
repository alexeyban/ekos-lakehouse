"""`python -m demo_book tour|render` — EKOS RFC 0218."""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

from . import render as render_mod


def pairs(rules: list[str]) -> list[tuple[str, str]]:
    out = []
    for r in rules:
        old, sep, new = r.partition("=")
        if not sep or not old:
            raise SystemExit(f"--replace takes OLD=NEW, not {r!r}")
        out.append((old, new))
    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="demo_book")
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("tour", help="walk the console's pages into a capture directory")
    t.add_argument("--console", required=True, help="the console's URL")
    t.add_argument("--workspace", required=True)
    t.add_argument("--out", required=True, type=Path)
    t.add_argument("--token-env", default="EKOS_CONSOLE_TOKEN_VALUE",
                   help="the variable holding a console token with the write role")
    t.add_argument("--bin", default=os.environ.get("EKOS_BIN", "ekos-lakehouse"),
                   help="the binary for the read commands (its facts)")
    t.add_argument("--state", type=Path, help="the project's state directory (default: from EKOS_HOME)")
    t.add_argument("--act", action="append", default=[], choices=["docs", "test-db", "reconcile", "edit-undo"],
                   help="also run this action (repeatable); nothing a later step depends on")
    t.add_argument("--mart", default="sales_by_month", help="the mart --act reconcile reconciles")
    for x in (t,):
        x.add_argument("--replace", action="append", default=[], metavar="OLD=NEW",
                       help="shorten a local path or account name on the screenshots (repeatable, in order)")
    r = sub.add_parser("render", help="write the book from a capture")
    r.add_argument("--capture", required=True, type=Path)
    r.add_argument("--out", required=True, type=Path)
    r.add_argument("--pdf", type=Path, help="also print the book to this PDF")
    r.add_argument("--replace", action="append", default=[], metavar="OLD=NEW",
                   help="shorten a local path or account name in the text and logs (repeatable, in order)")
    r.add_argument("--allow-missing", action="store_true",
                   help="exit 0 although the capture lacks something (listed on stderr)")
    a = ap.parse_args(argv)

    if a.cmd == "tour":
        from .tour import default_state, tour

        token = os.environ.get(a.token_env)
        if not token:
            ap.error(f"set {a.token_env}")
        cap = tour(console=a.console, token=token, ws=a.workspace, out=a.out, binary=a.bin,
                   state=a.state or default_state(a.workspace), acts=tuple(a.act), mart=a.mart,
                   replace=pairs(a.replace))
        print(f"captured {len(cap['shots'])} screenshot(s), {len(cap['runs'])} run(s) → {a.out}")
        for k, v in cap["shot_errors"].items():
            print(f"  not captured: {k}: {v}", file=sys.stderr)
        for k, v in cap["acts"].items():
            print(f"  {k}: {v}")
        return 1 if cap["shot_errors"] or any(v != "succeeded" for v in cap["acts"].values()) else 0

    done = render_mod.render(a.capture, a.out, pairs(a.replace))
    print(f"{len(done.pages)} page(s) → {a.out}")
    if a.pdf:
        from .pdf import write_pdf

        write_pdf(a.out, done.pages, a.pdf)
        print(f"PDF → {a.pdf}")
    for m in done.missing:
        print(f"  missing: {m}", file=sys.stderr)
    return 0 if not done.missing or a.allow_missing else 1


if __name__ == "__main__":
    sys.exit(main())

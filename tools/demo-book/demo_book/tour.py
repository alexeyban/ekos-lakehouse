"""The tour (EKOS RFC 0218 §2.2): a browser through the console's pages, step by step, into a capture
directory — screenshots, the facts the pages show (from the same read commands, as JSON), and the run log
of every console run and terminal run of the project.

It reads. With `--act` it also runs the few actions that change nothing a later step depends on: the
estate documentation (step 2), a connection test (step 1) and one mart's reconciliation (step 7). It
never clicks an approval, a sign-off or a build.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path

from .steps import STEPS, Checkpoint

LONG = 180_000
ACTS = ("docs", "test-db", "reconcile", "edit-undo")
# The change `edit-undo` makes and takes back: a filter on an approved mart (step 4) and a load mode
# (full, for a table that loads incrementally and has no change of its own) in step 5.
EDIT_MART = "category_region_sales"
EDIT_FILTER = "c_orderline.qtyordered > 0"
EDIT_TABLE = "ADEMPIERE.C_BPARTNER"


def command_path(argv: list[str]) -> str:
    """`lakehouse marts build` from `["lakehouse", "marts", "build", "--mart=x"]`."""
    words = []
    for a in argv:
        if a.startswith("-"):
            break
        words.append(a)
    return " ".join(words)


def stamp_of(name: str) -> str | None:
    """The time in a run log's file name (`20261026T134319.592Z-docs-generate.jsonl`)."""
    m = re.match(r"(\d{8})T(\d{6})(?:\.\d+)?Z-", name)
    if not m:
        return None
    t = datetime.strptime(m.group(1) + m.group(2), "%Y%m%d%H%M%S").replace(tzinfo=timezone.utc)
    return t.isoformat()


def terminal_runs(runs_dir: Path, logs: Path) -> list[dict]:
    """Runs started from a terminal: the project's `<state>/runs/*.jsonl` (EKOS RFC 0220)."""
    out = []
    if not runs_dir.is_dir():
        return out
    for f in sorted(runs_dir.glob("*.jsonl")):
        lines = [json.loads(x) for x in f.read_text(errors="replace").splitlines() if x.startswith("{")]
        if not lines:
            continue
        end = next((e for e in reversed(lines) if e.get("stage") == "run" and e.get("msg") == "finished"), None)
        rid = "t-" + f.stem
        (logs / f"{rid}.jsonl").write_text(f.read_text(errors="replace"))
        cmd = lines[0].get("cmd", "")
        out.append({
            "id": rid,
            "cmd": cmd,
            "argv": cmd.split(),
            "status": "running" if end is None else ("succeeded" if end.get("ok") else "failed"),
            "at": stamp_of(f.name),
            "source": "terminal",
            "log": f"logs/{rid}.jsonl",
        })
    return out


def run_cli(binary: str, ws: str, *args: str) -> dict | list | None:
    p = subprocess.run([binary, "--project", ws, *args], capture_output=True, text=True, timeout=600)
    if p.returncode != 0:
        return None
    try:
        return json.loads(p.stdout)
    except ValueError:
        return None


def facts(binary: str, ws: str) -> dict:
    f: dict = {
        "workflow": run_cli(binary, ws, "workflow", "status", "--json"),
        "requirements": run_cli(binary, ws, "lakehouse", "requirements", "show", "--json"),
        "extraction": run_cli(binary, ws, "lakehouse", "extraction", "show", "--json"),
        "tables": run_cli(binary, ws, "lakehouse", "status", "--json"),
        "marts": run_cli(binary, ws, "lakehouse", "marts", "show", "--json"),
    }
    if isinstance(f["extraction"], dict):
        f["extraction"].pop("tables", None)  # 849 rows the book does not quote
    f["mart_details"] = {}
    for m in (f["marts"] or {}).get("marts", []):
        d = run_cli(binary, ws, "lakehouse", "marts", "show", "--mart", m["mart"], "--json")
        sel = (d or {}).get("selected") or {}
        f["mart_details"][m["mart"]] = {"reconciliation": sel.get("reconciliation"), "state": sel.get("state")}
    return f


# Shortens local paths and account names in what the page shows, just before a screenshot (the page is
# reloaded before anything else reads it). One-word rules replace whole words only.
REPLACE_JS = """(rules) => {
  const esc = (s) => s.replace(/[.*+?^${}()|[\\]\\\\]/g, '\\\\$&');
  const rx = rules.map(([o, n]) => [new RegExp(/^\\w+$/.test(o) ? '\\\\b' + esc(o) + '\\\\b' : esc(o), 'g'), n]);
  const fix = (t) => rx.reduce((s, [r, n]) => s.replace(r, n), t);
  const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n = w.nextNode(); n; n = w.nextNode()) n.nodeValue = fix(n.nodeValue);
  for (const i of document.querySelectorAll('input, textarea')) i.value = fix(i.value);
}"""


class Tour:
    def __init__(self, page, base: str, ws: str, out: Path):
        self.page, self.base, self.ws, self.out = page, base.rstrip("/"), ws, out
        self.shots: dict[str, str] = {}
        self.errors: dict[str, str] = {}
        self.replace: list[tuple[str, str]] = []

    def url(self, path: str) -> str:
        return self.base + path.format(ws=self.ws)

    def wait_for(self, wait: str):
        if wait.startswith("aria:"):
            self.page.locator(f'[aria-label="{wait[5:]}"]').first.wait_for(timeout=LONG)
        else:
            self.page.get_by_text(wait).first.wait_for(timeout=LONG)

    def settle(self):
        self.page.wait_for_timeout(500)
        busy = self.page.locator("text=loading…")
        if busy.count() > 0:
            busy.first.wait_for(state="detached", timeout=LONG)

    def shoot(self, id: str, full_page: bool = False):
        if self.replace:
            self.page.evaluate(REPLACE_JS, [[o, n] for o, n in self.replace])
        name = f"shots/{id}.png"
        self.page.screenshot(path=str(self.out / name), full_page=full_page)
        self.shots[id] = name

    def checkpoint(self, cp: Checkpoint, prepare=None):
        if cp.id in self.shots:
            return  # taken by an action, showing its result
        try:
            self.page.goto(self.url(cp.path))
            self.wait_for(cp.wait)
            if cp.tab:
                self.page.get_by_role("tablist").get_by_role("button", name=cp.tab).click()
            if cp.row:
                self.page.get_by_role("cell", name=cp.row, exact=True).first.click()
            if prepare:
                prepare(self)
            self.settle()
            self.shoot(cp.id, cp.full_page)
        except Exception as e:  # noqa: BLE001 — a failed checkpoint is reported in the book
            self.errors[cp.id] = f"{type(e).__name__}: {str(e).splitlines()[0][:200]}"

    def wait_run(self) -> dict:
        self.page.wait_for_url("**/runs/**", timeout=LONG)
        rid = self.page.url.rstrip("/").split("/")[-1]
        run: dict = {}
        for _ in range(900):
            run = self.page.request.get(f"{self.base}/api/runs/{rid}").json()
            if run.get("status") not in ("queued", "running"):
                break
            time.sleep(1)
        return run


def act_docs(t: Tour) -> dict:
    """Generate, then Preview publish, on the Documentation page (EKOS RFC 0230)."""
    runs = []
    for button in ("Generate", "Preview publish"):
        t.page.goto(t.url("/w/{ws}/x/lakehouse/docs"))
        t.wait_for("Home page")
        runs.append(page_action(t, button))
    ok = all(r.get("status") == "succeeded" for r in runs)
    return {"status": "succeeded" if ok else "failed"}


def act_reconcile(t: Tour, mart: str) -> dict:
    t.page.goto(t.url("/w/{ws}/x/lakehouse/marts"))
    t.wait_for("Marts (click one)")
    t.page.get_by_role("cell", name=mart, exact=True).first.click()
    t.page.get_by_role("button", name=f"Reconcile: {mart}", exact=True).click()
    typed = t.page.locator('[aria-label^="type "][aria-label$=" to confirm"]')
    if typed.count() > 0:
        typed.fill(typed.get_attribute("aria-label")[len("type "):-len(" to confirm")])
    t.page.get_by_role("button", name="Run", exact=True).click()
    return t.wait_run()


def page_action(t: Tour, button: str, inputs: dict[str, str] | None = None) -> dict:
    """An extension page's action (EKOS RFC 0198/0226): its dialog, filled, run, waited for."""
    t.page.get_by_role("button", name=button, exact=True).click()
    for label, value in (inputs or {}).items():
        t.page.locator(f'[aria-label="{label}"]').fill(value)
    typed = t.page.locator('[aria-label^="type "][aria-label$=" to confirm"]')
    if typed.count() > 0:
        typed.fill(typed.get_attribute("aria-label")[len("type "):-len(" to confirm")])
    # An action with nothing to type or confirm runs on the click.
    run = t.page.get_by_role("button", name="Run", exact=True)
    try:
        run.wait_for(timeout=3000)
        run.click()
    except Exception:  # noqa: BLE001
        pass
    return t.wait_run()


def open_ext(t: Tour, page: str, wait: str, row: str, button: str):
    """Open an extension page and select `row` so that `button` shows. A name can be in several
    panels (a table with a change is also in the changes panel); only a selecting panel's row shows
    the selection's actions."""
    t.page.goto(t.url(f"/w/{{ws}}/x/lakehouse/{page}"))
    t.wait_for(wait)
    cells = t.page.get_by_role("cell", name=row, exact=True)
    cells.first.wait_for(timeout=LONG)
    target = t.page.get_by_role("button", name=button, exact=True)
    for i in range(cells.count()):
        cells.nth(i).click()
        try:
            target.wait_for(timeout=5000)
            return
        except Exception:  # noqa: BLE001 — not a selecting panel; try the next
            continue
    raise RuntimeError(f"{page}: no row {row} shows {button}")


def act_edit_undo(t: Tour, mart: str = EDIT_MART, table: str = EDIT_TABLE) -> dict:
    """A change and its undo in steps 4 and 5: two run logs each, and the approved requirement and
    plan are what they were (their digests are over the content, which is back)."""
    steps = (
        ("requirements", "Requirements (click one)", mart, f"Add a filter: {mart}", {"Filter": EDIT_FILTER}),
        ("requirements", "Requirements (click one)", mart, f"Remove a filter: {mart}",
         {"Filter, as written": EDIT_FILTER}),
        ("extraction", "Extraction", table, f"Load in full: {table}",
         {"Why (kept with the change)": "shown in the demo book, then undone"}),
        ("extraction", "Extraction", table, f"Undo changes: {table}", None),
    )
    runs = []
    for page, wait, row, button, inputs in steps:
        open_ext(t, page, wait, row, button)
        runs.append(page_action(t, button, inputs))
    ok = all(r.get("status") == "succeeded" for r in runs)
    return {"status": "succeeded" if ok else "failed"}


def act_test_db(t: Tour) -> dict:
    t.page.goto(t.url("/w/{ws}/sources"))
    t.wait_for("Sources")
    t.page.get_by_role("button", name="Test connection").first.click()
    t.page.locator('[aria-label="census vs compiled"], p.err').first.wait_for(timeout=LONG)
    ok = t.page.locator('[aria-label="census vs compiled"]').count() > 0
    # The result is gone once the page reloads: step 1's screenshot is this one.
    t.shoot("01-sources")
    return {"status": "succeeded" if ok else "failed"}


def console_runs(page, base: str, ws: str, logs: Path) -> list[dict]:
    out = []
    for r in page.request.get(f"{base}/api/runs?workspace={ws}&limit=500").json():
        params = r.get("params") or {}
        argv = params.get("argv") or []
        cmd = command_path(argv) if argv else r.get("command", "")
        events = page.request.get(f"{base}/api/runs/{r['id']}/events").json()
        log = None
        if events:
            log = f"logs/{r['id']}.jsonl"
            (logs / f"{r['id']}.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))
        out.append({
            "id": r["id"][:12],
            "cmd": cmd,
            "argv": argv or [cmd],
            "status": r.get("status"),
            "at": r.get("started_at") or r.get("created_at"),
            "source": "console",
            "log": log,
        })
    return out


def tour(*, console: str, token: str, ws: str, out: Path, binary: str, state: Path | None,
         acts: tuple[str, ...] = (), mart: str = "sales_by_month",
         replace: list[tuple[str, str]] | None = None) -> dict:
    from playwright.sync_api import sync_playwright

    for sub in ("shots", "logs"):
        (out / sub).mkdir(parents=True, exist_ok=True)
    acted: dict[str, dict] = {}
    with sync_playwright() as p:
        browser = p.chromium.launch(args=["--no-sandbox"])
        page = browser.new_page(viewport={"width": 1500, "height": 1100})
        page.set_default_timeout(LONG)
        page.goto(console.rstrip("/") + "/")
        page.locator('[aria-label="console token"]').fill(token)
        page.get_by_role("button", name="Sign in").click()
        page.get_by_text("Add a workspace").wait_for(timeout=LONG)
        t = Tour(page, console, ws, out)
        t.replace = list(replace or [])

        if "test-db" in acts:
            acted["test-db"] = act_test_db(t)
        if "docs" in acts:
            acted["docs"] = act_docs(t)
        if "edit-undo" in acts:
            acted["edit-undo"] = act_edit_undo(t)
        if "reconcile" in acts:
            acted["reconcile"] = act_reconcile(t, mart)

        for step in STEPS:
            for cp in step.checkpoints:
                t.checkpoint(cp)
        runs = console_runs(page, console.rstrip("/"), ws, out / "logs")
        browser.close()
    if state is not None:
        runs += terminal_runs(state / "runs", out / "logs")
    cap = {
        "console": console,
        "workspace": ws,
        "at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "binary": subprocess.run([binary, "--version"], capture_output=True, text=True).stdout.strip(),
        "acts": {k: v.get("status") for k, v in acted.items()},
        "shots": t.shots,
        "shot_errors": t.errors,
        "runs": runs,
        "facts": facts(binary, ws),
    }
    (out / "capture.json").write_text(json.dumps(cap, indent=1))
    return cap


def default_state(ws: str, tenant: str = "default") -> Path | None:
    home = os.environ.get("EKOS_HOME")
    return Path(home) / "data" / tenant / ws / "state" if home else None

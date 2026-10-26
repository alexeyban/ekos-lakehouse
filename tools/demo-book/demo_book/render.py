"""Render the book from a capture (EKOS RFC 0218 §2.3). Pure over the capture directory: no console,
no browser, no project.

Generated: screenshots, run-log excerpts, the model calls of every quoted run, the numbers (from the
capture's facts only) and each step's state on the bar. Written by a person: the manual sections, kept on
every render. Whatever is missing from the capture is said on the page and returned, so a caller can
refuse the book; a number is never carried over from an earlier render.
"""

from __future__ import annotations

import json
import re
import shutil
from dataclasses import dataclass, field
from pathlib import Path

from lakehouse_docs.md import code, front_matter, keep_manual, manual, table

from .steps import STEPS, Step

MAX_LINES = 20
MAX_RUNS = 4
PER_COMMAND = 2  # a change and its undo, a draft and its approval
QUIET = {"warning"}


@dataclass
class Rendered:
    pages: list[str] = field(default_factory=list)
    missing: list[str] = field(default_factory=list)


def replacer(rules: list[tuple[str, str]]):
    """Each `(old, new)` in order; an `old` that is one word is replaced as a whole word only."""
    compiled = [(re.compile(rf"\b{re.escape(o)}\b") if re.fullmatch(r"\w+", o) else None, o, n) for o, n in rules]

    def apply(text: str) -> str:
        for rx, o, n in compiled:
            text = rx.sub(n, text) if rx else text.replace(o, n)
        return text

    return apply


def matches(cmd: str, prefixes: tuple[str, ...]) -> bool:
    return any(cmd == p or cmd.startswith(p + " ") for p in prefixes)


def quoted_runs(step: Step, runs: list[dict]) -> list[dict]:
    """The newest runs of the step's commands (at most `PER_COMMAND` of one command, `MAX_RUNS` in
    all), in the order they ran."""
    seen: dict[str, int] = {}
    out = []
    for r in sorted(runs, key=lambda r: r.get("at") or "", reverse=True):
        cmd = r.get("cmd") or ""
        if matches(cmd, step.commands) and seen.get(cmd, 0) < PER_COMMAND:
            seen[cmd] = seen.get(cmd, 0) + 1
            out.append(r)
        if len(out) == MAX_RUNS:
            break
    return list(reversed(out))


def excerpt(events: list[dict]) -> list[dict]:
    """The lines worth reading: start, the stages, errors, the model calls, the outcome. Warnings are
    counted on the closing line already."""
    keep = [e for e in events if e.get("stage") not in QUIET]
    if len(keep) <= MAX_LINES:
        return keep
    head = keep[: MAX_LINES - 2]
    tail = [e for e in keep[MAX_LINES - 2 :] if e.get("stage") in ("llm", "run")][-2:]
    return head + tail


def model_calls(events: list[dict]) -> int | None:
    for e in events:
        if e.get("stage") == "llm":
            return int(e.get("count") or 0)
    return None


def read_events(path: Path) -> list[dict]:
    out = []
    if not path.is_file():
        return out
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if isinstance(e, dict):
            out.append(e)
    return out


def bar_line(step: Step, facts: dict) -> dict | None:
    for s in (facts.get("workflow") or {}).get("steps", []):
        if s.get("n") == step.n:
            return s
    return None


# -- the numbers, per kind of fact ------------------------------------------------------------------


def fact_review(facts: dict) -> str | None:
    s = bar_line(STEPS[2], facts)
    if not s or not s.get("subjects"):
        return None
    rows = [
        (x["subject"], x.get("items", 0), x.get("decided", 0), x.get("blocking", 0),
         (x.get("signoff") or {}).get("state", "none"))
        for x in s["subjects"]
    ]
    return table(["Subject", "Items", "Decided", "Blocking", "Sign-off"], rows)


def fact_requirements(facts: dict) -> str | None:
    r = facts.get("requirements")
    if not r:
        return None
    rows = [
        (code(m["mart"]), m.get("state", ""), m.get("terms", 0), m.get("questions", 0), m.get("source", ""),
         f"{m.get('approved_by') or ''} {m.get('approved_at') or ''}".strip())
        for m in r.get("marts", [])
    ]
    return table(["Mart", "State", "Terms", "Questions", "Described as", "Approved"], rows)


def fact_extraction(facts: dict) -> str | None:
    s = (facts.get("extraction") or {}).get("summary")
    if not s:
        return None
    rows = [
        ("incremental, deletes every run", s.get("incremental_every_run", 0)),
        ("incremental, deletes audited", s.get("incremental_audited", 0)),
        ("incremental, other", s.get("incremental_other", 0)),
        ("full", s.get("full", 0)),
        ("refused (type cannot carry the data)", s.get("refused", 0)),
        ("out of scope (step 3)", s.get("excluded", 0)),
        ("masked columns", s.get("masked_columns", 0)),
        ("operator changes", s.get("changes", 0)),
    ]
    by = f"approved by {s['approved_by']} at {s['approved_at']}" if s.get("approved_by") else "not approved"
    return table(["Tables", "Count"], rows) + f"\nPlan {code(s.get('digest', ''))}, {by}.\n"


def fact_tables(facts: dict) -> str | None:
    t = facts.get("tables")
    if t is None:
        return None
    counts: dict[tuple[str, str], int] = {}
    for x in t:
        k = (x.get("layer", "?"), x.get("state", "?"))
        counts[k] = counts.get(k, 0) + 1
    rows = [(layer, state, n) for (layer, state), n in sorted(counts.items())]
    return table(["Layer", "State", "Tables"], rows)


def fact_marts(facts: dict) -> str | None:
    m = facts.get("marts")
    if not m:
        return None
    out = table(["Mart", "State", "Last build"],
                [(code(x["mart"]), x.get("state", ""), x.get("last_build") or "") for x in m.get("marts", [])])
    rows = []
    for name, d in sorted((facts.get("mart_details") or {}).items()):
        for t in (d or {}).get("reconciliation") or []:
            rows.append((code(name), code(t.get("measure", "")), t.get("mart_total", ""),
                         t.get("silver_total", ""), t.get("equal", "")))
    if rows:
        out += "\nEach measure's total in the mart and the same total computed from silver:\n\n"
        out += table(["Mart", "Measure", "Mart total", "Silver total", ""], rows)
    return out


FACTS = {
    "workflow": lambda facts: None,  # the bar line is shown on every page
    "review": fact_review,
    "requirements": fact_requirements,
    "extraction": fact_extraction,
    "tables": fact_tables,
    "marts": fact_marts,
}


# -- pages -------------------------------------------------------------------------------------------


def step_page(step: Step, cap: dict, capdir: Path, book: Path, missing: list[str]) -> tuple[str, int | None]:
    facts = cap.get("facts") or {}
    body = [front_matter(f"Step {step.n} — {step.title}", step.n), f"# Step {step.n} — {step.title}\n"]
    bar = bar_line(step, facts)
    if bar:
        body.append(f"**On the step bar:** {bar.get('state', '?')} — {bar.get('detail', '')}\n")
    else:
        body.append("**On the step bar:** _not captured in this run._\n")
        missing.append(f"step {step.n}: the step bar")

    body.append("## What the operator sees\n")
    for cp in step.checkpoints:
        shot = (cap.get("shots") or {}).get(cp.id)
        if shot and (capdir / shot).is_file():
            (book / "images").mkdir(parents=True, exist_ok=True)
            shutil.copyfile(capdir / shot, book / "images" / f"{cp.id}.png")
            body.append(f"![{cp.caption}](images/{cp.id}.png)\n\n_{cp.caption}._\n")
        else:
            why = (cap.get("shot_errors") or {}).get(cp.id, "no screenshot")
            body.append(f"_{cp.caption}: not captured in this run ({why})._\n")
            missing.append(f"step {step.n}: screenshot {cp.id} ({why})")

    body.append("## What they click\n")
    body.append(manual(f"step{step.n}-click", step.click))
    body.append("\n## What happens behind it\n")
    body.append(manual(f"step{step.n}-behind", step.behind))

    body.append("\n## The run log\n")
    runs = quoted_runs(step, cap.get("runs") or [])
    total: int | None = None
    if not runs:
        body.append(
            "_No run of this step's commands was found in the console's history or the project's run logs "
            f"({', '.join(code(c) for c in step.commands)})._\n"
        )
        missing.append(f"step {step.n}: no run log")
    for r in runs:
        events = r.get("events") or []
        lines = excerpt(events)
        calls = model_calls(events)
        if calls is not None:
            total = (total or 0) + calls
        if lines:
            (book / "logs").mkdir(parents=True, exist_ok=True)
            (book / "logs" / f"{r['id']}.jsonl").write_text(
                "".join(json.dumps(e) + "\n" for e in lines), encoding="utf-8")
        where = "from the console" if r.get("source") == "console" else "from a terminal"
        argv = " ".join(r.get("argv") or [r.get("cmd", "")])
        body.append(
            f"**{code(argv)}** — {r.get('status', '?')}, {where}, {r.get('at', '?')}; model calls: "
            f"{calls if calls is not None else 'not recorded'}.\n"
        )
        if lines:
            body.append("```json\n" + "".join(json.dumps(e) + "\n" for e in lines) + "```\n")
        else:
            body.append("_This run left no structured log (it ran before EKOS RFC 0218)._\n")

    body.append("## What the operator gets\n")
    shown = False
    for kind in step.facts:
        text = FACTS[kind](facts)
        if text:
            body.append(text)
            shown = True
        elif kind != "workflow":
            body.append(f"_{kind}: not captured in this run._\n")
            missing.append(f"step {step.n}: facts {kind}")
    if not shown and step.facts == ("workflow",):
        body.append("The step's state above.\n")

    body.append("\n## Caveats\n")
    body.append(manual(f"step{step.n}-caveats", step.caveats))
    return "\n".join(body), total


def index_page(cap: dict, calls: dict[int, int | None]) -> str:
    facts = cap.get("facts") or {}
    rows = []
    for step in STEPS:
        bar = bar_line(step, facts) or {}
        c = calls.get(step.n)
        rows.append((f"[{step.n} {step.title}](step-{step.n}.md)", bar.get("state", "?"),
                     "—" if c is None else c))
    head = front_matter("The operator workflow, step by step", 0)
    return "\n".join([
        head,
        "# The operator workflow, step by step\n",
        manual("intro", "_What this book shows, on which estate, for whom._"),
        "\n## The steps\n",
        table(["Step", "State", "Model calls in the quoted runs"], rows),
        f"\nCaptured {cap.get('at', '?')} from the console at workspace {code(cap.get('workspace', ''))}"
        f" ({cap.get('binary', '?')}). Screenshots, run logs and numbers are regenerated on every capture; "
        "only the marked sections are written by a person."
        + (" Local paths and account names are shortened in the text, the logs and the screenshots."
           if cap.get("replaced") else "") + "\n",
    ])


def render(capdir: Path, book: Path, replace: list[tuple[str, str]] | None = None) -> Rendered:
    """`replace`: local paths and account names to shorten in the text and the logs (the tour does the
    same on the screenshots); the index says that it was done."""
    fix = replacer(replace or [])
    cap = json.loads(fix((capdir / "capture.json").read_text(encoding="utf-8")))
    cap["replaced"] = len(replace or [])
    for r in cap.get("runs") or []:
        if r.get("log") and (capdir / r["log"]).is_file():
            r["events"] = [json.loads(fix(json.dumps(e))) for e in read_events(capdir / r["log"])]
    book.mkdir(parents=True, exist_ok=True)
    for sub in ("images", "logs"):
        if (book / sub).is_dir():
            shutil.rmtree(book / sub)
    out = Rendered()
    calls: dict[int, int | None] = {}
    for step in STEPS:
        text, calls[step.n] = step_page(step, cap, capdir, book, out.missing)
        out.pages.append(write(book / f"step-{step.n}.md", text))
    out.pages.insert(0, write(book / "index.md", index_page(cap, calls)))
    return out


def write(path: Path, text: str) -> str:
    text = keep_manual(text, path)
    if not path.is_file() or path.read_text(encoding="utf-8") != text:
        path.write_text(text, encoding="utf-8")
    return path.name

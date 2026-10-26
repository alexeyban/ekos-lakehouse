import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from demo_book import render as r  # noqa: E402
from demo_book.steps import STEPS  # noqa: E402
from demo_book.tour import command_path, stamp_of, terminal_runs  # noqa: E402

PNG = bytes.fromhex("89504e470d0a1a0a0000000d4948445200000001000000010806000000"
                    "1f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082")


def ev(stage, msg, **kw):
    return {"stage": stage, "msg": msg, **kw}


def log_lines(*events):
    return "".join(json.dumps(e) + "\n" for e in events)


def capture(tmp: Path, marts=True) -> Path:
    cap = tmp / "cap"
    (cap / "shots").mkdir(parents=True, exist_ok=True)
    (cap / "logs").mkdir(exist_ok=True)
    (cap / "shots/07-marts.png").write_bytes(PNG)
    (cap / "logs/a.jsonl").write_text(log_lines(
        ev("run", "started", cmd="lakehouse marts reconcile"),
        ev("marts", "reconciled", mart="m", measure="total", mart_total="10.00", silver_total="10.00", equal=True),
        ev("warning", "noise"),
        ev("llm", "calls", count=0),
        ev("run", "finished", ok=True),
    ))
    (cap / "logs/b.jsonl").write_text(log_lines(ev("run", "started"), ev("llm", "calls", count=2),
                                                ev("run", "finished", ok=True)))
    steps = [{"n": s.n, "id": s.id, "title": s.title, "state": "done", "detail": f"detail {s.n}"} for s in STEPS]
    steps[2]["state"] = "partial"
    steps[2]["subjects"] = [{"subject": "scope", "items": 903, "decided": 3, "blocking": 0,
                             "signoff": {"state": "valid"}}]
    facts = {"workflow": {"steps": steps}, "requirements": {"marts": [{"mart": "m", "state": "approved", "terms": 3}]},
             "extraction": {"summary": {"full": 134, "incremental_every_run": 665, "digest": "abc"}},
             "tables": [{"layer": "bronze", "state": "late"}, {"layer": "bronze", "state": "late"}]}
    if marts:
        facts["marts"] = {"marts": [{"mart": "m", "state": "reconciled", "last_build": "ok"}]}
        facts["mart_details"] = {"m": {"reconciliation": [
            {"measure": "total", "mart_total": "10.00", "silver_total": "10.00", "equal": "equal"}]}}
    (cap / "capture.json").write_text(json.dumps({
        "workspace": "w", "at": "2026-10-26T12:00:00+00:00", "binary": "ekos-lakehouse 0.1",
        "shots": {"07-marts": "shots/07-marts.png"}, "shot_errors": {"01-sources": "Timeout"},
        "runs": [
            {"id": "a", "cmd": "lakehouse marts reconcile", "argv": ["lakehouse", "marts", "reconcile", "--mart=m"],
             "status": "succeeded", "at": "2026-10-26T11:00:00", "source": "console", "log": "logs/a.jsonl"},
            {"id": "old", "cmd": "lakehouse marts reconcile", "argv": [], "status": "succeeded",
             "at": "2026-10-25T11:00:00", "source": "console", "log": None},
            {"id": "oldest", "cmd": "lakehouse marts reconcile", "argv": [], "status": "failed",
             "at": "2026-10-24T11:00:00", "source": "console", "log": None},
            {"id": "b", "cmd": "lakehouse requirements draft", "argv": ["lakehouse", "requirements", "draft"],
             "status": "succeeded", "at": "2026-10-26T10:00:00", "source": "terminal", "log": "logs/b.jsonl"},
            {"id": "c", "cmd": "lakehouse martsx", "argv": [], "status": "succeeded", "at": "2026-10-27", "log": None},
        ],
        "facts": facts,
    }))
    return cap


def test_a_book_has_a_page_per_step_with_shots_logs_numbers_and_model_calls(tmp_path):
    book = tmp_path / "book"
    done = r.render(capture(tmp_path), book)
    assert done.pages == ["index.md"] + [f"step-{n}.md" for n in range(1, 8)]
    s7 = (book / "step-7.md").read_text()
    assert "![" in s7 and (book / "images/07-marts.png").is_file()
    assert "**On the step bar:** done — detail 7" in s7
    assert "| `m` | `total` | 10.00 | 10.00 | equal |" in s7
    assert "model calls: 0" in s7
    assert '"noise"' not in s7, "warnings are counted on the closing line, not quoted"
    assert "martsx" not in s7, "a command matches on whole words"
    assert s7.count("**`lakehouse marts reconcile") == 2, "the two newest runs of a command"
    assert "failed" not in s7
    assert s7.index("2026-10-25T11") < s7.index("2026-10-26T11"), "in the order they ran"
    assert (book / "logs/a.jsonl").is_file()
    s3 = (book / "step-3.md").read_text()
    assert "| scope | 903 | 3 | 0 | valid |" in s3
    index = (book / "index.md").read_text()
    assert "[4 Requirements](step-4.md) | done | 2 |" in index
    assert "[7 Marts](step-7.md) | done | 0 |" in index


def test_what_is_missing_is_said_and_returned(tmp_path):
    done = r.render(capture(tmp_path), tmp_path / "book")
    assert "step 1: screenshot 01-sources (Timeout)" in done.missing
    assert "step 1: no run log" in done.missing
    assert "not captured in this run (Timeout)" in (tmp_path / "book/step-1.md").read_text()


def test_the_narration_survives_and_numbers_never_carry_over(tmp_path):
    book = tmp_path / "book"
    r.render(capture(tmp_path), book)
    page = book / "step-7.md"
    text = page.read_text()
    default = STEPS[6].behind
    page.write_text(text.replace(default, "Written by the maintainer."))
    done = r.render(capture(tmp_path, marts=False), book)
    again = page.read_text()
    assert "Written by the maintainer." in again and default not in again
    assert "10.00 | 10.00" not in again
    assert "_marts: not captured in this run._" in again
    assert "step 7: facts marts" in done.missing


def test_a_long_log_keeps_its_start_and_its_end():
    events = [ev("run", "started")] + [ev("s", f"line {i}") for i in range(40)] + [
        ev("llm", "calls", count=0), ev("run", "finished", ok=True)]
    out = r.excerpt(events)
    assert len(out) <= r.MAX_LINES
    assert out[0]["msg"] == "started" and out[-1]["msg"] == "finished" and out[-2]["stage"] == "llm"


def test_tour_helpers(tmp_path):
    assert command_path(["lakehouse", "marts", "build", "--mart=x"]) == "lakehouse marts build"
    assert stamp_of("20261026T134319.592Z-docs-generate.jsonl") == "2026-10-26T13:43:19+00:00"
    assert stamp_of("notes.jsonl") is None
    runs = tmp_path / "runs"
    runs.mkdir()
    (runs / "20261026T134319.592Z-docs-generate.jsonl").write_text(log_lines(
        ev("run", "started", cmd="docs generate"), ev("run", "finished", ok=True)))
    logs = tmp_path / "logs"
    logs.mkdir()
    [t] = terminal_runs(runs, logs)
    assert (t["cmd"], t["status"], t["source"]) == ("docs generate", "succeeded", "terminal")
    assert (logs / f"{t['id']}.jsonl").is_file()


def test_local_paths_and_account_names_are_shortened_everywhere(tmp_path):
    cap = capture(tmp_path)
    data = json.loads((cap / "capture.json").read_text())
    data["facts"]["requirements"]["marts"][0]["approved_by"] = "legion"
    data["facts"]["workflow"]["steps"][1]["detail"] = "7 page(s) into /tmp/x/home3/data/docs; legionary"
    (cap / "capture.json").write_text(json.dumps(data))
    (cap / "logs/a.jsonl").write_text(log_lines(ev("run", "started", output="/tmp/x/home3/out"),
                                               ev("llm", "calls", count=0)))
    book = tmp_path / "book"
    r.render(cap, book, [("/tmp/x/home3", "~/.ekos"), ("legion", "operator")])
    text = "".join(p.read_text() for p in book.glob("*.md")) + (book / "logs/a.jsonl").read_text()
    assert "/tmp/x" not in text and "| operator " in text and "~/.ekos/data/docs" in text
    assert "legionary" in text, "a one-word rule replaces whole words only"
    assert "account names are shortened" in (book / "index.md").read_text()

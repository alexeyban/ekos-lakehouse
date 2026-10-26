# demo-book — the operator workflow as a generated book

EKOS RFC 0218. A script walks the EKOS console through the seven steps of the operator workflow and writes a
book: per step, the screen, what the operator clicks, the run log the console kept, what happens behind it,
what the operator gets and the caveats. Screenshots, log excerpts and numbers are regenerated on every run;
only the marked sections are written by a person, and regeneration keeps them (the lakehouse-docs
`<!-- ekos:manual -->` convention).

```
python -m demo_book tour   --console URL --workspace ID --out capture/   # browser → capture
python -m demo_book render --capture capture/ --out book/ [--pdf book.pdf] # capture → book
```

## Install

```bash
pip install -r tools/demo-book/requirements.txt && python -m playwright install chromium
```

## Tour

Against a running console, signed in with a token that has the write role (`EKOS_CONSOLE_TOKEN_VALUE`):

```bash
cd tools/demo-book
EKOS_CONSOLE_TOKEN_VALUE=… EKOS_BIN=ekos-lakehouse EKOS_HOME=~/.ekos \
  python -m demo_book tour --console http://127.0.0.1:8765 --workspace my-project --out ../../capture
```

It opens each step's page, photographs it (`shots/`), saves every run log of the workspace — console runs
from `/api/runs`, terminal runs from the project's `runs/` directory (`logs/`) — and the facts the pages show,
from the same read commands (`workflow status`, `lakehouse requirements|extraction|marts show`,
`lakehouse status`, all `--json`) into `capture.json`.

**It reads.** `--act` adds actions that leave the project as it was, so their run logs can be quoted:

| `--act` | What it runs from the console |
|---|---|
| `test-db` | Sources → **Test connection** (step 1's screenshot then shows the census) |
| `docs` | Documentation → **Generate**, then **Preview publish** (step 2; the preview sends nothing) |
| `edit-undo` | Requirements: add a filter, remove it; Extraction: load a table in full, undo (steps 4 and 5; the approved requirement and plan are back, digests unchanged) |
| `reconcile` | Marts → **Reconcile** one mart (`--mart`, default `sales_by_month`; step 7) |

It never clicks an approval, a sign-off, a merge or a build.

`--replace OLD=NEW` (repeatable, in order) shortens local paths and account names on the screenshots; pass
the same rules to `render`. A one-word rule replaces whole words only.

## Render

```bash
python -m demo_book render --capture ../../capture --out book --pdf book/demo-book.pdf
```

`index.md` and `step-1.md` … `step-7.md`, with `images/` and `logs/` (the quoted excerpts). The pages follow
the lakehouse-docs page format, so `tools/lakehouse-docs/publish/*.py --docs book` publishes them; that is the
operator's decision, never part of a render.

Whatever the capture lacks (a screenshot, a step without a run log, a fact) is said on the page and listed
on stderr, and the render exits 1 unless `--allow-missing`. A number is never carried over from an earlier
render.

## Tests

```bash
python -m pytest tools/demo-book/tests
```

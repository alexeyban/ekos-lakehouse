# Devlog 21 — Step 2 in the console: generate and publish the estate documentation (EKOS RFC 0230)

**Date:** 2026-10-26
**PRs:** none (direct to `main`); EKOS devlog_309
**Branch:** `main`

---

## Summary

Step 2 was the only step of the operator workflow without a console page: EKOS generated the estate pages, but there
was no store to pick, no Publish, and an installed binary could not find the Python publishers. Now
`[extensions.lakehouse.docs]` names the store, `lakehouse docs show|generate|publish` run it, the publishers are
compiled into the binary, step 2 is done only when the latest generation is published, and the **Documentation**
page has Generate, Preview publish and Publish. On iDempiere: 1,060 pages generated and previewed against the
GitHub wiki (1,061 files, nothing sent); browser run 17/17. The real publish waits for the maintainer.

---

## What was built

| Component | Change |
|---|---|
| `docs.rs` | `DocsSettings`/`Target`, `publisher_args`, `destination`, `step`, `Published`, the publisher check, `cmd` |
| `settings.rs` | `docs` sub-table |
| `lib.rs` | `lakehouse docs show` (read) / `generate` (write) / `publish [--dry-run]` (external) |
| `workflow.rs` | step 2 from the estate record and the latest real publish |
| `console.rs` | the Documentation page |
| `tools/demo-book` | step 2's checkpoint and `--act docs` on the Documentation page |

## Decisions
- **Publishers embedded with `include_str!`**, written to `<state>/lakehouse/docs/publish/` per run: an installed
  binary needs no source tree, and runs the publisher version it was built with.
- **Credentials stay with the publishers' own variables**; the page shows their names, never values.
- **A preview never completes step 2**; a publish completes it only for the generation it published.
- **`sidebar`** passes the GitHub publisher's `--sidebar keep`: by default it replaces the wiki's `_Sidebar.md`, which
  on a shared wiki (EKOS's own) is not what a project should do.

## Knowledge Captured
- The console's Markdown view shows raw HTML as text: the Home panel strips the `<!-- ekos:manual -->` markers.
- `rustfmt` reflows a long `assert_eq!` array one element per line — scripted edits must match the formatted text.

## Files Changed

| File | Change summary |
|---|---|
| `crates/lakehouse-ext/src/{docs,settings,lib,workflow,console}.rs` | the above |
| `tools/demo-book/` | step 2 on the Documentation page |
| `README.md` | Documentation section |

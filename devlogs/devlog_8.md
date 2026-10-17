# Devlog 8 — public, built from EKOS main, and the GitHub path run for real

**Date:** 2026-10-17
**PRs:** none (pushed to `main`)
**Branch:** main → github.com/alexeyban/ekos-lakehouse (public, MIT)
**Design:** EKOS RFC 0194 (§ "Found while building it", GitHub path)

---

## Summary

The repository is public and builds against EKOS `main` by git (EKOS pushed with RFC 0191, the
seam v2), with the local path patch kept commented out for working against a sibling checkout.
The schema-change pull-request path, untested until now, was run end to end on a private sandbox
pipeline repository (`alexeyban/ekos-lakehouse-sandbox-pipeline`); it had four bugs, all fixed.

---

## Publishing

### What was done
- Content audit before creating the repository: 68 files, 10 commits by one author, no local
  paths, no credentials (the sandbox's `*-local-only` values are compose defaults), nothing from
  the private binary-recovery workspace.
- `[patch."https://github.com/alexeyban/EKOS"]` commented out; `Cargo.lock` now pins
  54 EKOS crates at `35fad70`; the third-party lock entries did not change. Gates green.
- `gh repo create --public`; the first push made `master` (git's default here) — renamed to
  `main` and made the default before anyone could clone.

### Decisions
- EKOS by branch `main` for now; pin a tag for the first Lakehouse release.
- The sandbox pipeline repository is private: generated output for a demo database, not
  something to show on the public profile.

---

## The GitHub path

### Problem
`sync` with `git-remote` had never run against a real host.

### Found and fixed (`crates/lakehouse-ext/src/sync.rs`)

| Bug | Fix |
|---|---|
| `gh pr create --body-file proposals/<id>.md` read the main checkout; the file exists only on the branch | `ensure_pull_request` reads the body with `git show <branch>:proposals/<id>.md` |
| a request that failed to open was never retried (the branch then counted as current) | every run makes sure an open request exists for each proposal (`gh pr list --head`) |
| a merge on the remote was never seen: the table stayed paused | `sync` first fast-forwards the base branch from the remote (`--ff-only`; anything else is an error naming it) |
| safe-change commits stayed local | pushed after committing |
| merged proposal branches stayed on the remote | deleted with the local one (an unmerged one: closes its request) |

Also in the generator: `__pycache__/`, `*.pyc` and dbt's `.user.yml` were committed into the
pipeline repository on every generate (Dagster writes the `.pyc` when it loads
`definitions.py`). They are ignored now, and `commit_all` untracks files an updated ignore
covers.

### Verified (sandbox: Pagila project, live stack)
- `RENAME COLUMN name TO category_name` → proposal → PR #1 with the change table, evidence
  (attnum 2) and impact (silver `public__category.name`, its `not_null` test); a second `sync`
  found the same request.
- PR merged on GitHub → next `sync`: "the source matches the deployed plan", `category` healthy.
- `ADD COLUMN slug text` → applied, committed, pushed; no request.

---

## Knowledge Captured
- A proposal's files live on its branch only (built in a temporary worktree): anything run in
  the main checkout must read them through git, not the filesystem.
- "Branch exists and is current" is not "request exists": idempotence has to be checked against
  the host, not inferred from local state.
- With a remote, approval happens there. A command that decides from `HEAD` must bring `HEAD`
  up to date first, and only by fast-forward — EKOS never merges.
- `gh repo create --source . --push` pushes the current branch under its own name; check it is
  `main` before publishing.

---

## Files Changed
| File | Change summary |
|---|---|
| `Cargo.toml`, `Cargo.lock` | EKOS by git (`main`); path patch commented out |
| `crates/lakehouse-ext/src/sync.rs` | `ensure_pull_request`, pull before sync, push safe commits, delete remote proposal branches, untrack newly ignored files |
| `crates/bronze/src/render.rs`, `crates/silver/src/lib.rs` | ignore `__pycache__/`, `*.pyc`, `.user.yml` |
| `README.md` | what `sync` does with a remote |
| `devlogs/devlog_8.md` | this |

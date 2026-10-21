# lakehouse-docs — documentation for an EKOS Lakehouse pipeline

Generates a complete documentation set — Markdown pages with Mermaid diagrams — from what
ekos-lakehouse already knows, and publishes it to **Confluence**, an **Azure DevOps wiki** or a
**GitHub wiki**.

The generator never connects to a database. It reads the deployed plan
(`lakehouse.lock.json`), the generated dbt project, gold mart descriptions, proposals, the
pipeline repository's git history and the JSON evidence of `ekos-lakehouse lakehouse … --json`.
Every sentence states only what those say; where only a person knows (business purpose, owners,
access policy), the page carries a **manual section** that regeneration keeps.

```
pipeline/                      ← the pipeline repository ekos-lakehouse generates
  lakehouse.lock.json            source facts + bronze plan
  dbt/models/silver|gold/…       silver and gold models, tests
  marts/*.md  proposals/*.md     gold descriptions, change proposals
  docs/                        ← written by this tool
    _evidence/                   collected JSON (not published)
    index.md  business/  architecture/  source/  target/  lineage/
    quality/  governance/  operations/
```

## Install

```bash
pip install -r tools/lakehouse-docs/requirements.txt
npm i -g @mermaid-js/mermaid-cli     # only for Confluence diagrams as images (--mermaid image)
```

## Generate

From the EKOS project directory (the one with `ekos.toml`; the pipeline is `./pipeline`):

```bash
export PYTHONPATH=$PWD/tools/lakehouse-docs     # from the ekos-lakehouse checkout
python -m lakehouse_docs collect  --project ~/my-lakehouse --check          # status, tables, proposals, check, dbt artifacts
python -m lakehouse_docs generate --project ~/my-lakehouse                  # → ~/my-lakehouse/pipeline/docs
python -m lakehouse_docs all      --project ~/my-lakehouse --check --strict # both; exit 1 if lineage has gaps
```

* `collect` is read-only by default. `--check` runs `lakehouse check --lint` (it records its
  metrics in the ledger, as any check run does). `--reconcile` runs a fresh `lakehouse reconcile`
  — it queries the source and Trino and plants a control row, so use it deliberately.
* dbt's `run_results.json` / `manifest.json` / `catalog.json` are picked up from `dbt/target/`
  when present (after `dbt build` / `dbt docs generate`): test results, real column types.
* Copy `lakehouse-docs.toml.example` to `<project>/lakehouse-docs.toml` for business context,
  owners, consumers, glossary, environments and SLAs. Every key is optional.
* Re-running is safe: manual sections are preserved, unchanged files are not rewritten, generated
  pages that no longer apply (a dropped table) are deleted, hand-written pages are never touched.

## What is generated

| Section | Pages | Built from |
|---|---|---|
| **Home** | at-a-glance numbers, contents, provenance (commit, evidence time) | everything |
| **Business description** | purpose, scope, owners/RACI, consumers, business questions (gold marts), KPI definitions; **glossary** | `lakehouse-docs.toml`, mart descriptions, table comments |
| **Architecture** | context diagram, components, layer contracts, load strategy, run sequence diagram, schema-change handling, environments, security; **decision log (ADRs)** incl. per-table load decisions with evidence, masking decisions, refused tables | lock settings, `lakehouse tables --json` |
| **Source** | table inventory, **ER diagram** per schema, one page per table: columns, keys, FKs in/out, neighbourhood ER, load mode + evidence, downstream columns | lock (catalog facts) |
| **Target** | bronze (meta columns, modes, type-mapping & lossiness); silver index, **silver ER diagram**, one page per model (columns, tests + last result, column lineage, SQL); gold index and one page per mart (business description, definitions, term resolution, anti-invention result, approval state, column lineage diagram, tests incl. measure reconciliation, SQL) | dbt project, marts, proposals, run results |
| **Lineage** | table-level flow diagram source → bronze → silver → gold; **column lineage / STTM** (every source column through every layer, every gold column back to source with its transform chain) | lock mapping + SQL parsed with sqlglot (Trino dialect) |
| **Data quality & reconciliation** | scorecard; reconciliation per table (tiers, divergences, planted control); DQ rule catalogue, coverage, failing tests; code-quality metrics; health per table/layer; sign-off section | `reconcile`, `check`, `status` JSON, dbt `run_results.json` |
| **Governance** | **data dictionary** (silver + gold columns with source and PII flags); **PII register** (classifier candidates vs approved masks and where each column flows); credentials | lock, silver SQL lineage |
| **Operations** | runbook (schedules, commands, action per health state); **SLAs and generated data contracts**; schema-change proposals and applied changes; change log | lock, proposals, git log |

## Publish

All three publishers are standalone scripts in `publish/` (shared helper `wiki_common.py`). They
take the docs folder, mirror its hierarchy (`<dir>/index.md` is the parent of the other pages in
`<dir>`), rewrite links between pages, are **idempotent** (unchanged pages are not touched), have
`--dry-run`, and `--prune` removes pages they published earlier that no longer exist locally —
never pages they did not create.

### Confluence (Cloud or Data Center)

```bash
export CONFLUENCE_URL=https://acme.atlassian.net/wiki
export CONFLUENCE_USER=me@acme.com CONFLUENCE_API_TOKEN=…     # Cloud
# export CONFLUENCE_PAT=…                                      # Data Center / Server
python publish/publish_confluence.py --docs ~/my-lakehouse/pipeline/docs \
    --space DATA --parent-id 123456 --title-prefix "Pagila · " --prune
```

* Markdown → storage format: tables, code macros, real page links, local images as attachments.
* Mermaid: `--mermaid image` (default; PNGs rendered with `mmdc`, content-addressed attachments),
  `--mermaid macro --mermaid-macro <name>` for a Mermaid Marketplace app, or `--mermaid code`.
* Titles are unique per Confluence space: use `--title-prefix`. A same-titled page the tool did
  not create is never overwritten (`--adopt` to take it over). Published pages get the label
  `ekos-docs`; the content hash is kept in the version message.

### Azure DevOps wiki

```bash
export AZDO_ORG_URL=https://dev.azure.com/acme AZDO_PROJECT=Data AZDO_WIKI=Data.wiki AZDO_PAT=…
python publish/publish_azure_devops_wiki.py --docs ~/my-lakehouse/pipeline/docs \
    --root-path "/Lakehouse/Pagila" --prune            # code wiki: add --branch main
```

* PAT scope: **Wiki (Read & Write)**. Parent pages of `--root-path` must exist.
* Mermaid fences become Azure DevOps `::: mermaid` blocks. If your organisation's renderer does not
  draw a diagram type (older ones lack `erDiagram`), use `--mermaid image` (PNG attachments).

### GitHub wiki

```bash
export GITHUB_TOKEN=…            # contents: write on the repository
python publish/publish_github_wiki.py --docs ~/my-lakehouse/pipeline/docs \
    --repo acme/pagila-lakehouse --prefix Pagila --prune
```

* Create the wiki's first page once in the GitHub UI (GitHub creates the wiki repo only then).
* GitHub wikis are flat: pages become `<Prefix>-<Title>.md`, the hierarchy goes to `_Sidebar.md`.
  Mermaid renders natively. Without `--prefix` the top page is `Home`.

## In CI

The workflow lives in the **pipeline repository**; the tool is fetched from ekos-lakehouse.

GitHub Actions — regenerate and publish after every change to the pipeline:

```yaml
name: docs
on: { push: { branches: [main] }, workflow_dispatch: {} }
jobs:
  docs:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }                 # the change log needs history
      - uses: actions/setup-python@v5
        with: { python-version: "3.12" }
      - run: |
          git clone --depth 1 https://github.com/alexeyban/ekos-lakehouse "$RUNNER_TEMP/lh"
          pip install -r "$RUNNER_TEMP/lh/tools/lakehouse-docs/requirements.txt"
      # Run evidence (status/check/reconcile JSON) is produced where the ledger lives: schedule
      # `python -m lakehouse_docs collect` there (e.g. a Dagster job) and commit docs/_evidence.
      - run: PYTHONPATH="$RUNNER_TEMP/lh/tools/lakehouse-docs" python -m lakehouse_docs generate --project . --pipeline . --strict
      - run: python "$RUNNER_TEMP/lh/tools/lakehouse-docs/publish/publish_github_wiki.py" --docs docs --repo "${{ github.repository }}" --prune
        env: { GITHUB_TOKEN: "${{ secrets.WIKI_TOKEN }}" }
```

Azure Pipelines — the same, publishing to the project wiki:

```yaml
steps:
  - checkout: self
    fetchDepth: 0
  - script: |
      git clone --depth 1 https://github.com/alexeyban/ekos-lakehouse $(Agent.TempDirectory)/lh
      pip install -r $(Agent.TempDirectory)/lh/tools/lakehouse-docs/requirements.txt
      PYTHONPATH=$(Agent.TempDirectory)/lh/tools/lakehouse-docs python -m lakehouse_docs generate --project . --pipeline .
      python $(Agent.TempDirectory)/lh/tools/lakehouse-docs/publish/publish_azure_devops_wiki.py --docs docs --root-path "/Lakehouse/Pagila" --prune
    env:
      AZDO_ORG_URL: $(System.CollectionUri)
      AZDO_PROJECT: $(System.TeamProject)
      AZDO_WIKI: $(System.TeamProject).wiki
      AZDO_PAT: $(WIKI_PAT)
```

## Tests

```bash
cd tools/lakehouse-docs && pip install pytest responses && python -m pytest -q tests
```

The fixture pipeline (`tests/fixtures/pipeline`) was produced by the repository's own bronze and
silver generators from a Pagila subset (`tests/fixtures/sources.json`), plus one gold mart. The
evidence files are hand-made test data. Publishers are tested against in-memory fakes of the
Confluence and Azure DevOps REST APIs and a local bare git repository for the GitHub wiki;
every Mermaid diagram is rendered with `mmdc` when it is installed.

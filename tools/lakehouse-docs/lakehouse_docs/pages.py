"""Every documentation page, rendered from the loaded project, its lineage and its evidence.

Pages are plain Markdown with a YAML front matter (``title``, ``order``) and Mermaid diagrams in
```` ```mermaid ```` fences. The folder layout is the page hierarchy: ``x/index.md`` is the page
for folder ``x`` and every other file in ``x/`` is its child. The publishers rely on nothing else.

Rule kept throughout: a sentence states only what the plan, the dbt project, the git history or
the collected evidence says. Where only a person knows (business purpose, owners, access policy),
the page carries a manual section that regeneration preserves.
"""

from __future__ import annotations

import datetime as dt
import re
from collections import Counter, defaultdict
from dataclasses import dataclass, field

from .config import DocsConfig
from .lineage import Lineage, Node, table_edges
from .md import (
    code,
    esc,
    front_matter,
    m_id,
    m_label,
    m_type,
    manual,
    mermaid,
    rel_link,
    slug,
    table,
)
from .project import Mart, Model, Project, SourceTable, Test


@dataclass
class Page:
    path: str            # relative to the docs root, e.g. "source/tables/public-film.md"
    title: str
    order: int
    body: str

    def render(self) -> str:
        return front_matter(self.title, self.order) + f"\n# {self.title}\n\n" + self.body.rstrip() + "\n"


@dataclass
class Site:
    p: Project
    cfg: DocsConfig
    lin: Lineage
    pages: list[Page] = field(default_factory=list)

    # ── paths ──────────────────────────────────────────────────────────────────────────────────
    @staticmethod
    def source_page(t: SourceTable) -> str:
        return f"source/tables/{slug(t.qualified)}.md"

    @staticmethod
    def model_page(m: Model) -> str:
        if m.layer == "gold":
            return f"target/gold/marts/{slug(m.name.removeprefix('gold__'))}.md"
        if m.layer == "silver":
            return f"target/silver/tables/{slug(m.name)}.md"
        return f"target/other-models.md#{slug(m.name)}"

    def link(self, here: str, there: str, text: str) -> str:
        anchor = ""
        if "#" in there:
            there, anchor = there.split("#", 1)
            anchor = "#" + anchor
        return f"[{esc(text)}]({rel_link(here, there)}{anchor})"

    def add(self, path: str, title: str, order: int, body: str) -> None:
        self.pages.append(Page(path, title, order, body))

    # ── evidence helpers ───────────────────────────────────────────────────────────────────────
    def health(self, table: str, layer: str) -> dict | None:
        for h in self.p.evidence.status or []:
            if h.get("table") == table and h.get("layer") == layer:
                return h
        return None

    def recon(self, qualified: str) -> dict | None:
        for r in self.p.evidence.reconcile or []:
            if r.get("table") == qualified:
                return r
        return None

    def test_result(self, model: Model, t: Test) -> dict | None:
        results = self.p.evidence.test_results()
        if not results:
            return None
        prefix = f"{t.name}_{model.name}" + (f"_{t.column}" if t.column else "")
        hits = [v for k, v in results.items() if k.startswith(prefix)]
        if t.name == "relationships" and len(hits) > 1:
            to = re.search(r"ref\('([^']+)'\)", str(t.args.get("to", "")))
            if to:
                hits = [v for k, v in results.items() if k.startswith(prefix) and to.group(1) in k] or hits
        return hits[0] if hits else None

    @staticmethod
    def status_badge(s: str | None) -> str:
        return {
            "pass": "✅ pass", "success": "✅ success", "ok": "✅ ok", "green": "✅ green",
            "healthy": "✅ healthy", "fail": "❌ fail", "error": "❌ error", "failed": "❌ failed",
            "diverged": "❌ diverged", "failing": "❌ failing", "warn": "⚠️ warn",
            "unverified": "⚠️ unverified", "late": "⚠️ late", "schema_pending": "⏸ schema pending",
            "skipped": "⏭ skipped",
        }.get((s or "").lower(), s or "—")

    def evidence_note(self) -> str:
        e = self.p.evidence
        if not any([e.status, e.check, e.reconcile, e.run_results]):
            return (
                "> **No run evidence collected yet.** Run `python -m lakehouse_docs collect` "
                "(see the docs tool README) and regenerate: this page then shows the latest "
                "reconciliation, test, quality and health results.\n\n"
            )
        when = e.collected_at or "unknown time"
        return f"> Evidence collected {when}" + (f" at pipeline commit `{e.meta.get('head')}`" if e.meta.get("head") else "") + ".\n\n"

    # ── build ──────────────────────────────────────────────────────────────────────────────────
    def build(self) -> list[Page]:
        self.home()
        self.business()
        self.glossary()
        self.architecture()
        self.decisions()
        self.source_overview()
        self.source_er()
        for t in self.p.tables:
            self.source_table(t)
        self.target_overview()
        self.bronze()
        self.silver_overview()
        self.silver_er()
        for m in self.p.silver:
            self.silver_model(m)
        self.gold_overview()
        for m in self.p.gold:
            self.gold_mart(m)
        if self.p.other_models:
            self.other_models()
        self.lineage_overview()
        self.column_lineage()
        self.quality_overview()
        self.reconciliation()
        self.data_tests()
        self.code_quality()
        self.health_page()
        self.governance_overview()
        self.data_dictionary()
        self.pii()
        self.operations_overview()
        self.runbook()
        self.sla()
        self.schema_changes()
        self.changelog()
        return self.pages

    # ══ Home ═══════════════════════════════════════════════════════════════════════════════════
    def home(self) -> None:
        p, cfg = self.p, self.cfg
        n_cols = sum(len(t.columns) for t in p.tables)
        tests = sum(len(m.all_tests()) for m in p.models.values())
        recon = Counter((r.get("verdict") or "").lower() for r in p.evidence.reconcile or [])
        results = p.evidence.test_results()
        failed = sum(1 for r in results.values() if r.get("status") not in ("pass", "success"))
        health = Counter(h.get("state") for h in p.evidence.status or [])
        rows = [
            ("Source tables (planned / refused)", f"{len(p.tables)} / {len(p.refused)}"),
            ("Source columns", n_cols),
            ("Silver models", len(p.silver)),
            ("Gold marts", len(p.gold)),
            ("Other dbt models (e.g. Pentaho translations)", len(p.other_models)),
            ("Generated dbt tests", tests),
        ]
        if results:
            rows.append(("dbt tests in the last run (failed / run)", f"{failed} / {len(results)}"))
        if recon:
            rows.append(("Reconciliation (green / diverged / unverified)",
                         f"{recon.get('green', 0)} / {recon.get('diverged', 0)} / {recon.get('unverified', 0)}"))
        if health:
            rows.append(("Health states", ", ".join(f"{k}: {v}" for k, v in sorted(health.items()))))
        if p.proposals:
            rows.append(("Proposals in the repository", len(p.proposals)))

        here = "index.md"
        sections = [
            ("business/index.md", "Business description", "purpose, scope, owners, consumers, KPIs"),
            ("business/glossary.md", "Glossary", "business terms and where each is implemented"),
            ("architecture/index.md", "Architecture", "components, data flow, layers, conventions"),
            ("architecture/decisions.md", "Architecture decisions", "platform and per-table decisions with their evidence"),
            ("source/index.md", "Source", "tables, keys, relations, ER diagram"),
            ("target/index.md", "Target", "bronze, silver and gold — tables, relations, ER diagrams"),
            ("lineage/index.md", "Lineage", "table- and column-level, source to gold"),
            ("quality/index.md", "Data quality & reconciliation", "checks, tests, code quality, health"),
            ("governance/index.md", "Governance", "data dictionary, PII and masking"),
            ("operations/index.md", "Operations", "runbook, SLAs, schema changes, change log"),
        ]
        body = f"{esc(cfg.purpose) or ''}\n\n" if cfg.purpose else ""
        body += manual("home-intro", "_A one-paragraph introduction for readers new to this data product._")
        body += "\n## At a glance\n\n" + table(["", "Value"], rows)
        body += "\n## Contents\n\n" + table(
            ["Section", "What it covers"], [(self.link(here, path, name), what) for path, name, what in sections]
        )
        prov = [
            ("Pipeline repository", code(str(p.root.name))),
            ("Pipeline commit", code(p.head or "not a git repository")),
            ("Plan format", p.lock_format),
            ("Source catalog (Trino)", code(p.settings.get("source_catalog"))),
            ("Target catalog (Iceberg)", code(p.settings.get("target_catalog"))),
            ("Evidence collected", p.evidence.collected_at or "not collected"),
            ("Generated", dt.date.today().isoformat()),
        ]
        body += "\n## Provenance\n\nEvery page is generated from the deployed plan (`lakehouse.lock.json`), the generated dbt project, mart descriptions, proposals, git history and collected run evidence. Edit only inside the marked manual sections — regeneration keeps them and replaces everything else.\n\n"
        body += table(["", ""], prov)
        self.add("index.md", f"{cfg.project} — data documentation", 0, body)

    # ══ Business ═══════════════════════════════════════════════════════════════════════════════
    def business(self) -> None:
        p, cfg, here = self.p, self.cfg, "business/index.md"
        body = "## Purpose\n\n" + manual("business-purpose", cfg.purpose or "_Why this data product exists: the decisions it supports and for whom._")
        body += "\n## Scope\n\n" + manual("business-scope", (cfg.scope or "_What is in scope._") + "\n\n**Out of scope:** " + (cfg.out_of_scope or "_What is deliberately not covered._"))
        body += "\n## Source system\n\n" + manual(
            "business-source", cfg.source_description or f"_What {cfg.source_system} is used for in the business, and who owns it._"
        )
        body += f"\nThe pipeline reads **{len(p.tables)} tables** from {esc(cfg.source_system)} (Trino catalog {code(p.settings.get('source_catalog'))})"
        body += f"; {len(p.refused)} table(s) are refused because a column has no faithful landing ([details]({rel_link(here, 'source/index.md')}#refused-tables)).\n" if p.refused else ".\n"
        described = [t for t in p.tables if t.comment]
        if described:
            body += "\nBusiness entities, as the source describes them:\n\n" + table(
                ["Entity", "Description"], [(self.link(here, self.source_page(t), t.qualified), t.comment) for t in described]
            )
        body += "\n## Owners and stakeholders\n\n"
        if cfg.owners:
            body += table(["Role", "Name", "Contact"], [(o.role, o.name, o.contact) for o in cfg.owners])
        body += manual("business-raci", "_Data owner, data steward, technical owner, approvers of gold marts (stakeholder + data engineer, RFC 0195)._")
        body += "\n## Consumers\n\n"
        if cfg.consumers:
            body += table(["Consumer", "Uses", "Notes"], [(c.get("name", ""), c.get("uses", ""), c.get("notes", "")) for c in cfg.consumers])
        body += manual("business-consumers", "_Reports, dashboards, applications and teams that read the gold marts._")
        body += "\n## Business questions answered (gold marts)\n\n"
        if p.marts:
            rows = []
            for mart in p.marts:
                model = p.models.get(f"gold__{mart.mart}")
                name = self.link(here, self.model_page(model), mart.mart) if model else mart.mart
                status = "built" if model else ("questions open" if mart.questions else "described, not drafted")
                rows.append((name, mart.prose.split("\n")[0], ", ".join(mart.grain), status))
            body += table(["Mart", "Question", "Grain", "Status"], rows)
            body += "\n### KPI definitions\n\n" + table(
                ["KPI", "Mart", "Definition (as written by the stakeholder)"],
                [(code(k), mart.mart, v) for mart in p.marts for k, v in mart.measures.items()],
            )
        else:
            body += "_No gold mart descriptions yet. A stakeholder writes one in `marts/<name>.md`; `lakehouse gold draft` turns it into a proposal._\n"
        self.add(here, "Business description", 10, body)

    def glossary(self) -> None:
        p, cfg, here = self.p, self.cfg, "business/glossary.md"
        rows: list[tuple[str, str, str, str]] = []
        for term, definition in sorted(cfg.glossary.items()):
            rows.append((f"**{esc(term)}**", definition, "glossary (lakehouse-docs.toml)", ""))
        for mart in p.marts:
            model = p.models.get(f"gold__{mart.mart}")
            where = self.link(here, self.model_page(model), mart.mart) if model else mart.mart
            for section, items in (("dimension", mart.dimensions), ("measure", mart.measures)):
                for term, written in items.items():
                    col = model.column(term) if model else None
                    impl = col.description.split(" — ", 1)[-1] if col and " — " in col.description else ""
                    rows.append((f"**{esc(term)}** ({section})", written, f"mart {where}", code(impl) if impl else ""))
        for t in p.tables:
            if t.comment:
                rows.append((f"**{esc(t.table)}** (entity)", t.comment, self.link(here, self.source_page(t), t.qualified), ""))
        body = "Business terms, where each is defined and how it is implemented. A term used in a gold mart must resolve to a silver column or a confirmed concept (RFC 0195); unresolved terms are asked back to the author, never guessed.\n\n"
        body += table(["Term", "Definition", "Defined in", "Implemented as"], rows)
        body += "\n## Additional terms\n\n" + manual("glossary-extra", "_Terms the business uses that are not yet in a mart description._")
        self.add(here, "Glossary", 11, body)

    # ══ Architecture ═══════════════════════════════════════════════════════════════════════════
    def architecture(self) -> None:
        p, cfg, here = self.p, self.cfg, "architecture/index.md"
        s = p.settings
        svc = (p.evidence.tables or {}).get("settings", {}) if p.evidence.tables else {}
        modes = Counter(t.mode for t in p.tables)
        body = "## Context\n\n"
        body += mermaid(
            "flowchart LR\n"
            f'  SRC[("{m_label(cfg.source_system)}<br/>source OLTP")]\n'
            '  subgraph DP["Data plane"]\n'
            '    TRINO["Trino<br/>postgresql + iceberg connectors"]\n'
            '    POL["Polaris<br/>Iceberg REST catalog"]\n'
            '    S3[("S3 object storage<br/>Iceberg data + metadata")]\n'
            '    DAG["Dagster<br/>schedules bronze SQL + dbt-trino"]\n'
            "  end\n"
            '  subgraph CP["Control plane"]\n'
            '    EKOS["EKOS + Lakehouse extension<br/>plan · generate · reconcile · check · sync"]\n'
            '    GIT[("Pipeline Git repository<br/>SQL · dbt · Dagster · proposals")]\n'
            '    LEDGER[("EKOS ledger<br/>facts with evidence")]\n'
            "  end\n"
            '  USERS["Consumers<br/>BI · analysts · agents (MCP read-only)"]\n'
            "  SRC -- reads catalog --> EKOS\n"
            "  EKOS -- commits / proposals --> GIT\n"
            "  GIT -- code --> DAG\n"
            "  DAG -- runs --> TRINO\n"
            "  SRC -- rows --> TRINO\n"
            "  TRINO --> POL\n  POL --> S3\n"
            "  DAG -- run-report --> EKOS\n"
            "  EKOS --> LEDGER\n"
            "  TRINO -- gold marts --> USERS\n"
        )
        body += "EKOS is in the control plane only: it reads the source catalog, generates pipeline code into Git and records every run and check as facts. Trino moves the rows, Dagster schedules runs, and people approve what is lossy, breaking or business-facing (merge = approval).\n\n"
        body += "## Components\n\n" + table(
            ["Component", "Role", "Endpoint / setting"],
            [
                ("Source", cfg.source_system, code(svc.get("source") or s.get("source_catalog"))),
                ("Trino", "query engine; moves source rows into Iceberg", code(svc.get("trino"))),
                ("Polaris", "Iceberg REST catalog", code(svc.get("polaris"))),
                ("Object storage", "Iceberg data and metadata files", code(svc.get("storage"))),
                ("Dagster", "orchestrator; bronze assets, dbt assets, maintenance", code(svc.get("dagster"))),
                ("dbt-trino", "silver and gold transforms with tests", "`dbt/` in the pipeline repo"),
                ("EKOS ledger", "facts: plan, runs, reconciliations, checks, changes", "project `.ekos/`"),
            ],
        )
        body += "\n## Layers\n\n" + table(
            ["Layer", "Location", "Built by", "Contract"],
            [
                ("Bronze", f"{code(s.get('target_catalog'))} · namespace {code(s.get('namespace_prefix', 'bronze_') + '<schema>')}",
                 "generated Trino SQL, one asset per table", "faithful copy; append-only with `_ekos_*` columns; reconciled to source every run"),
                ("Silver", "schema `silver_<schema>`, model `<schema>__<table>`",
                 "generated dbt-trino, no model in the loop", "latest row per key, deletes removed, approved PII masked; key/not-null/FK/enum tests"),
                ("Gold", "schema `gold`, model `gold__<mart>`",
                 "drafted from a written description, approved by merge", "every term resolved; grain uniqueness and measure reconciliation tests"),
            ],
        )
        body += "\n## Load strategy\n\n" + table(
            ["Mode", "Tables", "Meaning"],
            [
                ("full", modes.get("full", 0), "`CREATE OR REPLACE TABLE … AS SELECT` each run, one atomic commit"),
                ("incremental", modes.get("incremental", 0), "rows at or after the highest loaded watermark; key-set diff appends delete tombstones"),
            ],
        )
        body += f"\nBronze schedule: {code(s.get('bronze_schedule'))} · Iceberg maintenance: {code(s.get('maintenance_schedule'))}. Per-table choices and their evidence are in [Bronze]({rel_link(here, 'target/bronze.md')}).\n"
        body += "\n## One pipeline run\n\n"
        body += mermaid(
            "sequenceDiagram\n"
            "  autonumber\n"
            "  participant D as Dagster\n  participant T as Trino\n  participant I as Iceberg (Polaris + S3)\n"
            "  participant E as EKOS\n  participant G as Git\n"
            "  D->>E: lakehouse sync (source changes)\n"
            "  E->>G: commit safe changes / open proposals\n"
            "  D->>T: bronze SQL per table (full or incremental)\n"
            "  T->>I: write snapshot\n"
            "  D->>E: run-report (rows, snapshot id, status)\n"
            "  D->>E: lakehouse reconcile (V1–V3 + planted control)\n"
            "  D->>T: dbt build (silver, gold, tests)\n"
            "  D->>E: run-report --dbt-results\n"
            "  E-->>D: status / alert transitions\n"
        )
        body += "\n## Source schema changes\n\nDetected at the start of every run by comparing the live catalog with the deployed plan. Safe classes (new table, new nullable column, safe widening) are committed automatically; breaking classes (rename, drop, narrowing, new NOT NULL, key change) become a branch `ekos/change/<table>/<id>` with impact attached. See [Schema changes](" + rel_link(here, "operations/schema-changes.md") + ").\n"
        body += "\n## Environments\n\n"
        if cfg.environments:
            body += table(["Environment", "Purpose", "Notes"], [(e.get("name", ""), e.get("purpose", ""), e.get("notes", "")) for e in cfg.environments])
        body += manual("architecture-environments", "_Dev / test / prod: where each runs, how code is promoted, who can deploy._")
        body += "\n## Security\n\nCredentials never appear in configuration or the pipeline repository: settings name environment variables (for example `source-password-env`, `EKOS_LH_PII_KEY`). Agents read through MCP tools that refuse merge, approve, rerun and production writes. See [PII and security](" + rel_link(here, "governance/pii-and-security.md") + ").\n\n"
        body += "## Notes\n\n" + manual("architecture-notes", "_Integration points, network zones, capacity, cost and anything a new engineer must know._")
        self.add(here, "Architecture description", 20, body)

    def decisions(self) -> None:
        p, here = self.p, "architecture/decisions.md"
        adrs = [
            ("Load engine: Trino connectors", "EKOS must stay out of the data path.", "Trino `postgresql` → `iceberg` connectors move rows; EKOS only generates SQL.", "Large tables load through one connection unless key-range splitting applies."),
            ("Change capture: watermark + key-set diff", "No Kafka/Debezium in a self-serve install.", "Incremental only where a BEFORE UPDATE trigger proves the watermark; deletes by key-set diff.", "Updates without a proven watermark need full loads; reconciliation catches divergence."),
            ("Transforms: dbt-trino", "Readable, testable code that EKOS can compile back for lineage.", "Silver generated deterministically; gold drafted from descriptions.", "Hand edits are detected as drift and kept, not overwritten."),
            ("Orchestrator: Dagster (generated project)", "Asset model matches medallion layers.", "Dagster project generated outside the core.", "Replaceable later (e.g. Airflow)."),
            ("Catalog: Polaris (Iceberg REST)", "Open REST catalog.", "Bronze, silver and gold are Iceberg tables in Polaris.", "Pin versions; REST catalogs evolve fast."),
            ("Gold authorship: draft, check, approve", "Business meaning is never inferred unconfirmed.", "A model drafts gold SQL under an anti-invention check; stakeholder + data engineer approve by merge.", "Vague descriptions come back as questions."),
        ]
        body = "Decisions this data product rests on. Platform decisions come from RFC 0190; per-table decisions are recorded by the generator with their evidence.\n\n## Platform decisions\n\n"
        for i, (title, ctx, dec, cons) in enumerate(adrs, 1):
            body += f"### ADR-{i:03d} — {title}\n\n- **Status:** accepted\n- **Context:** {ctx}\n- **Decision:** {dec}\n- **Consequences:** {cons}\n\n"
        body += "## Per-table load decisions\n\n" + table(
            ["Table", "Mode", "Watermark", "Deletes", "Evidence", "Notes"],
            [
                (self.link(here, self.source_page(t), t.qualified), t.mode, code(t.watermark), "key-set diff" if t.keyset_deletes else "—",
                 "; ".join(t.mode_evidence), "; ".join(t.mode_notes))
                for t in p.tables
            ],
        )
        silver_masked = self._masked()
        if silver_masked:
            body += "\n## Masking decisions\n\nApproved in `[extensions.lakehouse.pii]`; candidates from the name classifier are reported, never applied without approval.\n\n"
            body += table(["Column", "Method"], [(code(k), v) for k, v in sorted(silver_masked.items())])
        if p.refused:
            body += "\n## Refused tables\n\nA table with any column that has no faithful Iceberg landing is refused whole — bronze never drops a column.\n\n"
            body += table(["Table", "Reasons"], [(r.qualified, "; ".join(r.reasons)) for r in p.refused])
        body += "\n## Project decisions\n\n" + manual("adr-project", "_Add decisions specific to this project (ADR-1xx): context, decision, consequences, date, approver._")
        self.add(here, "Architecture decisions", 21, body)

    def _masked(self) -> dict[str, str]:
        """``schema.table.column`` → ``hmac`` | ``null``, read from the silver SQL's lineage."""
        out: dict[str, str] = {}
        for m in self.p.silver:
            t = self.p.table_by_model(m.name)
            if not t:
                continue
            for c in m.columns:
                for e in self.lin.up.get(Node("silver", m.name, c.name), []):
                    tr = e.transform.upper()
                    if "HMAC" in tr:
                        out[f"{t.qualified}.{c.name}"] = "hmac"
                    elif e.kind == "constant" and "NULL" in tr:
                        out[f"{t.qualified}.{c.name}"] = "null"
        return out

    # ══ Source ═════════════════════════════════════════════════════════════════════════════════
    def source_overview(self) -> None:
        p, here = self.p, "source/index.md"
        referenced = Counter(fk.ref for t in p.tables for fk in t.foreign_keys)
        rows = []
        for t in p.tables:
            pii = [c.name for c in t.columns if c.pii_candidate]
            rows.append((
                self.link(here, self.source_page(t), t.qualified), len(t.columns), ", ".join(t.primary_key) or "⚠️ none",
                len(t.foreign_keys), referenced.get(t.qualified, 0), t.mode, ", ".join(pii), t.comment or "",
            ))
        body = f"Source: **{esc(self.cfg.source_system)}**, read through Trino catalog {code(p.settings.get('source_catalog'))}. "
        body += f"[ER diagram]({rel_link(here, 'source/er-diagram.md')}) · [Column lineage]({rel_link(here, 'lineage/column-lineage.md')})\n\n"
        body += manual("source-description", self.cfg.source_description or "_What the source system is, who owns it, how it is accessed, and any known data issues._")
        body += "\n## Tables\n\n" + table(["Table", "Columns", "Primary key", "FKs out", "Referenced by", "Load", "PII candidates", "Description"], rows)
        nopk = [t.qualified for t in p.tables if not t.primary_key]
        if nopk:
            body += f"\n> ⚠️ Tables without a primary key keep every row in silver and cannot use key-set deletes: {', '.join(nopk)}.\n"
        body += "\n## Refused tables\n\n" + (table(["Table", "Reasons"], [(r.qualified, "<br>".join(r.reasons)) for r in p.refused]) if p.refused else "_None: every source table has a faithful landing._\n")
        self.add(here, "Source", 30, body)

    def _er(self, entities: dict[str, list[tuple[str, str, str, str]]], rels: list[tuple[str, str, str, bool]]) -> str:
        """entities: id → [(type, name, keys, comment)], rels: (child, parent, label, optional)."""
        lines = ["erDiagram"]
        for ent, attrs in entities.items():
            if attrs:
                lines.append(f"  {m_id(ent)} {{")
                for typ, name, keys, comment in attrs:
                    line = f"    {m_type(typ)} {m_id(name)}"
                    if keys:
                        line += f" {keys}"
                    if comment:
                        line += f' "{m_label(comment)}"'
                    lines.append(line)
                lines.append("  }")
            else:
                lines.append(f"  {m_id(ent)} {{\n    key none\n  }}")
        for child, parent, label, optional in rels:
            card = "}o--o|" if optional else "}o--||"
            lines.append(f'  {m_id(child)} {card} {m_id(parent)} : "{m_label(label)}"')
        return mermaid("\n".join(lines))

    def _source_entity(self, t: SourceTable, keys_only: bool) -> list[tuple[str, str, str, str]]:
        out = []
        fk_cols = {c for fk in t.foreign_keys for c in fk.columns}
        for c in t.columns:
            keys = ",".join(k for k, on in (("PK", c.name in t.primary_key), ("FK", c.name in fk_cols)) if on)
            if keys_only and not keys:
                continue
            out.append((c.pg_type, c.name, keys, "" if keys_only else (c.comment or "")))
        return out

    def _source_rels(self, tables: list[SourceTable], names: set[str]) -> list[tuple[str, str, str, bool]]:
        rels = []
        for t in tables:
            for fk in t.foreign_keys:
                if fk.ref in names:
                    optional = any(not (t.column(c) and t.column(c).not_null) for c in fk.columns)
                    rels.append((t.qualified, fk.ref, ", ".join(fk.columns), optional))
        return rels

    def source_er(self) -> None:
        p, here = self.p, "source/er-diagram.md"
        by_schema: dict[str, list[SourceTable]] = defaultdict(list)
        for t in p.tables:
            by_schema[t.schema].append(t)
        body = "Entities are source tables; relationships are declared foreign keys (child `}o--||` parent; `o|` when the FK column is nullable). Each table page shows its neighbourhood with every column.\n\n"
        for schema, tables in sorted(by_schema.items()):
            keys_only = len(tables) > self.cfg.er_max_tables
            names = {t.qualified for t in tables}
            body += f"## Schema `{schema}`\n\n"
            if keys_only:
                body += f"_{len(tables)} tables: keys only. Full columns are on each table page._\n\n"
            ents = {t.qualified: self._source_entity(t, keys_only) for t in tables}
            body += self._er(ents, self._source_rels(tables, names))
            cross = [(t.qualified, fk.ref) for t in tables for fk in t.foreign_keys if fk.ref not in names]
            if cross:
                body += "\nForeign keys to other schemas: " + ", ".join(f"{a} → {b}" for a, b in cross) + "\n"
        self.add(here, "Source ER diagram", 31, body)

    def source_table(self, t: SourceTable) -> None:
        p, here = self.p, self.source_page(t)
        referenced_by = [(o, fk) for o in p.tables for fk in o.foreign_keys if fk.ref == t.qualified]
        body = (t.comment + "\n\n") if t.comment else ""
        body += manual(f"source-{slug(t.qualified)}", "_Business meaning of this table, known quirks, data owner._")
        body += "\n## Columns\n\n"
        rows = []
        for c in t.columns:
            fk = t.fk_for(c.name)
            fk_txt = self.link(here, self.source_page(p.table(fk.ref)), f"{fk.ref}.{fk.ref_columns[fk.columns.index(c.name)]}") if fk and p.table(fk.ref) else (f"{fk.ref}" if fk else "")
            rows.append((
                c.attnum or "", code(c.name), code(c.pg_type), "NOT NULL" if c.not_null else "null",
                "yes" if c.has_default else "", "PK" if c.name in t.primary_key else "", fk_txt,
                ", ".join(c.enum_labels), c.pii_candidate or "", c.comment or "",
            ))
        body += table(["#", "Column", "Type", "Null", "Default", "Key", "References", "Allowed values", "PII candidate", "Comment"], rows)
        body += "\n## Relations\n\n"
        rel_rows = [("→ references", self.link(here, self.source_page(p.table(fk.ref)), fk.ref) if p.table(fk.ref) else fk.ref,
                     ", ".join(fk.columns) + " → " + ", ".join(fk.ref_columns), fk.name) for fk in t.foreign_keys]
        rel_rows += [("← referenced by", self.link(here, self.source_page(o), o.qualified), ", ".join(fk.columns) + " → " + ", ".join(fk.ref_columns), fk.name) for o, fk in referenced_by]
        body += table(["Direction", "Table", "Columns", "Constraint"], rel_rows)
        neigh = [t] + [p.table(fk.ref) for fk in t.foreign_keys if p.table(fk.ref)] + [o for o, _ in referenced_by]
        uniq = list({x.qualified: x for x in neigh}.values())
        if len(uniq) > 1:
            ents = {x.qualified: self._source_entity(x, keys_only=x is not t) for x in uniq}
            body += "\n" + self._er(ents, self._source_rels(uniq, {x.qualified for x in uniq}))
        body += "\n## Load into bronze\n\n" + table(["", ""], [
            ("Bronze table", code(t.bronze)),
            ("Mode", t.mode + (f" on watermark {code(t.watermark)}" if t.watermark else "")),
            ("Hard deletes", "key-set diff (tombstones)" if t.keyset_deletes else "replaced with the table" if t.mode == "full" else "not detected"),
            ("Evidence", "<br>".join(t.mode_evidence)),
            ("Notes", "<br>".join(t.mode_notes)),
        ])
        if t.renames:
            body += "\nApproved renames (applied in place): " + ", ".join(f"`{r['from']}` → `{r['to']}`" for r in t.renames) + "\n"
        r = self.recon(t.qualified)
        if r:
            body += f"\nLast reconciliation: **{self.status_badge(r.get('verdict'))}** — control {'fired' if r.get('control_fired') else 'MISSED' if r.get('control_fired') is False else 'n/a'}. [Details]({rel_link(here, 'quality/reconciliation.md')})\n"
        body += "\n## Downstream\n\n"
        down_rows = []
        for c in t.columns:
            leaves = self.lin.downstream(Node("source", t.qualified, c.name))
            silver = [n for n in leaves if n.layer == "silver"]
            gold = [n for n in leaves if n.layer == "gold"]
            other = [n for n in leaves if n.layer not in ("bronze", "silver", "gold")]
            down_rows.append((code(c.name), ", ".join(code(n.label) for n in silver), ", ".join(code(n.label) for n in gold), ", ".join(code(n.label) for n in other)))
        model = p.models.get(t.model)
        if model:
            body += f"Silver model: {self.link(here, self.model_page(model), model.name)}"
            users = p.consumers_of(model.name)
            if users:
                body += " · read by " + ", ".join(self.link(here, self.model_page(u), u.name) for u in users)
            body += "\n\n"
        body += table(["Column", "Silver", "Gold", "Other models"], down_rows)
        self.add(here, t.qualified, 100 + p.tables.index(t), body)

    # ══ Target ═════════════════════════════════════════════════════════════════════════════════
    def target_overview(self) -> None:
        p, here = self.p, "target/index.md"
        body = "The target is a medallion lakehouse of Apache Iceberg tables.\n\n" + table(
            ["Layer", "Objects", "Page"],
            [
                ("Bronze", f"{len(p.tables)} tables", self.link(here, "target/bronze.md", "Bronze")),
                ("Silver", f"{len(p.silver)} models, {len(p.snapshots)} snapshots", self.link(here, "target/silver/index.md", "Silver")),
                ("Gold", f"{len(p.gold)} marts", self.link(here, "target/gold/index.md", "Gold")),
            ] + ([("Other", f"{len(p.other_models)} models", self.link(here, "target/other-models.md", "Other models"))] if p.other_models else []),
        )
        body += "\n" + self._flow(compact=True)
        self.add(here, "Target", 40, body)

    def bronze(self) -> None:
        p, here = self.p, "target/bronze.md"
        body = "A faithful, append-only copy of each source table. Nothing is cleaned here. Each row carries:\n\n" + table(
            ["Column", "Meaning"],
            [("`_ekos_loaded_at`", "when the row was loaded"), ("`_ekos_run_id`", "the run that loaded it"),
             ("`_ekos_source_lsn`", "source position at load time"), ("`_ekos_op`", "`delete` for a key-set-diff tombstone")],
        )
        body += f"\nIceberg maintenance (snapshot expiry, compaction, orphan files) runs on {code(p.settings.get('maintenance_schedule'))}.\n\n## Tables\n\n"
        rows = []
        for t in p.tables:
            not_exact = [f"{c.name} ({c.lossiness})" for c in t.columns if c.lossiness != "exact"]
            not_cmp = [c.name for c in t.columns if not c.compared]
            h = self.health(t.qualified, "bronze")
            rows.append((code(t.bronze), self.link(here, self.source_page(t), t.qualified), t.mode, code(t.watermark),
                         ", ".join(not_exact), ", ".join(not_cmp), self.status_badge(h.get("state")) if h else "—"))
        body += table(["Bronze table", "Source", "Mode", "Watermark", "Not exact", "Not compared", "Health"], rows)
        body += "\n## Type mapping\n\nEvery PostgreSQL type maps to an Iceberg type with a lossiness class: `exact`, `widening`, `as_text`, `narrowing_safe` or `refused`.\n\n"
        maps = Counter((c.pg_type, c.iceberg_type, c.lossiness, c.rule) for t in p.tables for c in t.columns)
        body += table(["PostgreSQL", "Iceberg (Trino)", "Lossiness", "Rule", "Columns"], [(code(a), code(b), c, d, n) for (a, b, c, d), n in sorted(maps.items())])
        self.add(here, "Bronze", 41, body)

    def silver_overview(self) -> None:
        p, here = self.p, "target/silver/index.md"
        masked = self._masked()
        snaps = {re.sub(r".*ref\('([^']+)'\).*", r"\1", s.relation): s for s in p.snapshots}
        rows = []
        for m in sorted(p.silver, key=lambda x: x.name):
            t = p.table_by_model(m.name)
            mm = [k.rsplit(".", 1)[1] for k in masked if t and k.startswith(t.qualified + ".")]
            h = self.health(t.qualified, "silver") if t else None
            rows.append((self.link(here, self.model_page(m), m.name), code(m.relation), self.link(here, self.source_page(t), t.qualified) if t else "",
                         len(m.columns), len(m.all_tests()), "SCD2" if m.name in snaps else "", ", ".join(mm), self.status_badge(h.get("state")) if h else "—"))
        body = "Clean, conformed, tested: one model per bronze table, generated from source facts (RFC 0193). Latest version per key, deletes removed, approved PII masked. "
        body += f"[ER diagram]({rel_link(here, 'target/silver/er-diagram.md')})\n\n"
        body += table(["Model", "Relation", "Source", "Columns", "Tests", "History", "Masked", "Health"], rows)
        if p.snapshots:
            body += "\n## History (dbt snapshots)\n\n" + table(["Snapshot", "Of", "Strategy", "Key", "Table"], [(code(s.name), code(s.relation), s.strategy, code(s.unique_key), code(s.alias)) for s in p.snapshots])
        self.add(here, "Silver", 42, body)

    def _silver_rels(self, models: list[Model]) -> list[tuple[str, str, str, bool]]:
        names = {m.name for m in models}
        rels = []
        for m in models:
            for c in m.columns:
                for t in c.tests:
                    if t.name == "relationships":
                        to = re.search(r"ref\('([^']+)'\)", str(t.args.get("to", "")))
                        if to and to.group(1) in names:
                            optional = not any(x.name == "not_null" for x in c.tests)
                            rels.append((m.name, to.group(1), c.name, optional))
        return rels

    def _silver_entity(self, m: Model, keys_only: bool) -> list[tuple[str, str, str, str]]:
        t = self.p.table_by_model(m.name)
        pk = set(t.primary_key) if t else {c.name for c in m.columns if any(x.name == "unique" for x in c.tests)}
        fks = {c.name for c in m.columns if any(x.name == "relationships" for x in c.tests)}
        out = []
        for c in m.columns:
            keys = ",".join(k for k, on in (("PK", c.name in pk), ("FK", c.name in fks)) if on)
            if keys_only and not keys:
                continue
            out.append((c.data_type, c.name, keys, "" if keys_only else c.description))
        return out

    def silver_er(self) -> None:
        models = sorted(self.p.silver, key=lambda m: m.name)
        keys_only = len(models) > self.cfg.er_max_tables
        body = "Relationships are the `relationships` tests generated from declared foreign keys — the same constraints the source declares, now verified on every run.\n\n"
        body += self._er({m.name: self._silver_entity(m, keys_only) for m in models}, self._silver_rels(models))
        self.add("target/silver/er-diagram.md", "Silver ER diagram", 43, body)

    def _column_lineage_rows(self, here: str, m: Model) -> list[tuple[str, ...]]:
        rows = []
        for c in m.columns:
            roots = self.lin.roots(Node(m.layer, m.name, c.name))
            srcs = []
            for n, path in roots:
                t = self.p.table(n.relation)
                srcs.append(self.link(here, self.source_page(t), n.label) if t else n.label)
            transforms = []
            for e in self.lin.up.get(Node(m.layer, m.name, c.name), []):
                if e.transform != "pass-through":
                    transforms.append(code(e.transform))
            rows.append((code(c.name), "<br>".join(srcs), "<br>".join(transforms) or "pass-through"))
        return rows

    def silver_model(self, m: Model) -> None:
        p, here = self.p, self.model_page(m)
        t = p.table_by_model(m.name)
        body = (m.description + "\n\n") if m.description else ""
        info = [("Relation", code(m.relation)), ("File", code(m.path)), ("Materialized", m.materialized)]
        if t:
            info.insert(0, ("Source", self.link(here, self.source_page(t), t.qualified)))
            info.insert(1, ("Bronze", code(t.bronze)))
            info.append(("Key", ", ".join(t.primary_key) or "none — every row kept"))
        res = p.evidence.model_results().get(m.name)
        if res:
            info.append(("Last dbt run", f"{self.status_badge(res.get('status'))} in {res.get('execution_time', 0):.1f}s"))
        h = self.health(t.qualified, "silver") if t else None
        if h:
            info.append(("Health", self.status_badge(h.get("state")) + (f" — {h.get('cause')}" if h.get("cause") else "")))
        users = p.consumers_of(m.name)
        if users:
            info.append(("Read by", ", ".join(self.link(here, self.model_page(u), u.name) for u in users)))
        body += table(["", ""], info)
        lin = {r[0]: r for r in self._column_lineage_rows(here, m)}
        body += "\n## Columns\n\n" + table(
            ["Column", "Type", "Description", "Tests", "From source", "Transform"],
            [(code(c.name), code(c.data_type), c.description, ", ".join(x.describe() for x in c.tests), lin[code(c.name)][1], lin[code(c.name)][2]) for c in m.columns],
        )
        body += "\n## Tests\n\n" + self._tests_table(m)
        body += "\n## SQL\n\n```sql\n" + m.sql.strip() + "\n```\n"
        self.add(here, m.name, 200 + sorted(x.name for x in p.silver).index(m.name), body)

    def _tests_table(self, m: Model) -> str:
        rows = []
        for t in m.all_tests():
            r = self.test_result(m, t)
            res = self.status_badge(r.get("status")) + (f" ({r.get('failures')} rows)" if r and r.get("failures") else "") if r else "—"
            rows.append((code(t.column) if t.column else "_model_", t.describe(), t.rule or "", t.evidence or "", res))
        return table(["Column", "Test", "Rule", "Evidence", "Last result"], rows)

    def gold_overview(self) -> None:
        p, here = self.p, "target/gold/index.md"
        rows = []
        for m in sorted(p.gold, key=lambda x: x.name):
            mart = self._mart(m)
            h = self.health(f"gold.{m.name.removeprefix('gold__')}", "gold")
            rows.append((self.link(here, self.model_page(m), m.name.removeprefix("gold__")), (m.description or "").split("\n")[0],
                         ", ".join(mart.grain) if mart else "", ", ".join(m.refs), self.status_badge(h.get("state")) if h else "—"))
        body = "Marts built from written business descriptions (RFC 0195): every term resolves to a silver column or is asked back, nothing is invented, and each mart is approved by merge.\n\n"
        body += table(["Mart", "Question", "Grain", "Reads", "Health"], rows)
        pending = [mt for mt in p.marts if f"gold__{mt.mart}" not in p.models]
        if pending:
            body += "\n## Described, not yet built\n\n" + table(["Mart", "Description", "State"], [(mt.mart, mt.prose.split("\n")[0], "questions for the author" if mt.questions else "not drafted") for mt in pending])
        if p.gold:
            lines = ["flowchart LR"]
            for m in p.gold:
                lines.append(f'  {m_id(m.name)}[["{m.name}"]]')
                for r in m.refs:
                    lines.append(f'  {m_id(r)}["{r}"] --> {m_id(m.name)}')
            body += "\n## Marts and the silver models they read\n\n" + mermaid("\n".join(lines))
        self.add(here, "Gold", 44, body)

    def _mart(self, m: Model) -> Mart | None:
        return next((x for x in self.p.marts if f"gold__{x.mart}" == m.name), None)

    def gold_mart(self, m: Model) -> None:
        p, here = self.p, self.model_page(m)
        mart = self._mart(m)
        name = m.name.removeprefix("gold__")
        body = "## Business description\n\n" + ((mart.prose if mart else m.description) or "_No description._") + "\n\n"
        info = [("Model", code(m.name)), ("Relation", code(m.relation)), ("File", code(m.path))]
        if mart:
            info += [("Description file", code(mart.path)), ("Grain", ", ".join(mart.grain)), ("Filters", "<br>".join(mart.filters) or "—")]
        prop = next((x for x in p.proposals if x.id == f"gold-{name}"), None)
        open_branches = [b for b in ((p.evidence.proposals or {}).get("proposals") or []) if b.get("kind") == "gold mart" and name in (b.get("subject") or "")]
        info.append(("Approval", "⏳ proposal open: " + ", ".join(code(b.get("branch")) for b in open_branches) if open_branches else "merged (approved)" if prop or m else "—"))
        h = self.health(f"gold.{name}", "gold")
        if h:
            info.append(("Health", self.status_badge(h.get("state"))))
        body += table(["", ""], info)
        if mart:
            body += "\n## Definitions\n\n" + table(
                ["Term", "Kind", "Written definition", "Implemented as"],
                [(code(k), kind, v, code((m.column(k).description.split(" — ", 1)[-1]) if m.column(k) and " — " in m.column(k).description else ""))
                 for kind, items in (("dimension", mart.dimensions), ("measure", mart.measures)) for k, v in items.items()],
            )
        if prop:
            terms = re.search(r"## Terms\n\n(.*?)(?:\n## |\Z)", prop.body, re.S)
            joins = re.search(r"## Joins\n\n(.*?)(?:\n## |\Z)", prop.body, re.S)
            inv = re.search(r"## Anti-invention\n\n(.*?)(?:\n## |\Z)", prop.body, re.S)
            if terms:
                body += "\n## Term resolution (from the approved proposal)\n\n" + terms.group(1).strip() + "\n"
            if joins:
                body += "\n**Joins:** " + joins.group(1).strip() + "\n"
            if inv:
                body += "\n**Anti-invention check:** " + inv.group(1).strip() + "\n"
        if mart and mart.questions:
            body += "\n## Open questions for the author\n\n" + re.sub(r"^# .*\n", "", mart.questions).strip() + "\n"
        body += "\n## Column lineage\n\n" + table(["Column", "From source", "Transform"], self._column_lineage_rows(here, m))
        lines = ["flowchart LR"]
        for c in m.columns:
            dst = m_id(f"g_{c.name}")
            lines.append(f'  {dst}["{name}.{c.name}"]')
            for n, _ in self.lin.roots(Node("gold", m.name, c.name)):
                sid = m_id(f"s_{n.label}")
                lines.append(f'  {sid}("{n.label}") --> {dst}')
        body += "\n" + mermaid("\n".join(lines))
        body += "\n## Tests\n\n" + self._tests_table(m)
        recon_tests = sorted((p.root / "dbt" / "tests" / "gold").glob(f"*{name}*.sql")) if (p.root / "dbt" / "tests" / "gold").exists() else []
        if recon_tests:
            results = p.evidence.test_results()
            body += "\nSingular tests (measure reconciliation against silver):\n\n" + table(
                ["Test", "Last result"], [(code(x.stem), self.status_badge(results.get(x.stem, {}).get("status")) if x.stem in results else "—") for x in recon_tests]
            )
        body += "\n## SQL\n\n```sql\n" + m.sql.strip() + "\n```\n"
        body += "\n## Notes\n\n" + manual(f"gold-{slug(name)}", "_Known limitations, refresh expectations and consumers of this mart._")
        self.add(here, name, 300 + sorted(x.name for x in p.gold).index(m.name), body)

    def other_models(self) -> None:
        here = "target/other-models.md"
        body = "dbt models outside the generated silver and gold layers — for example Pentaho (Kettle) translations (RFC 0200).\n\n"
        for m in sorted(self.p.other_models, key=lambda x: x.name):
            body += f"## {m.name}\n\n" + table(["", ""], [("File", code(m.path)), ("Reads", ", ".join(m.refs + [f'{a}.{b}' for a, b in m.sources]))])
            body += "\n" + table(["Column", "From source", "Transform"], self._column_lineage_rows(here, m)) + "\n"
        self.add(here, "Other models", 49, body)

    # ══ Lineage ════════════════════════════════════════════════════════════════════════════════
    def _flow(self, compact: bool = False) -> str:
        edges = table_edges(self.p)
        nodes = sorted({n for e in edges for n in e})
        if compact or len(nodes) > self.cfg.lineage_max_nodes:
            # One node per layer group: source schema → bronze namespace → silver schema → gold
            def group(n: str) -> str:
                layer, rel = n.split(":", 1)
                if layer in ("source", "bronze"):
                    return f"{layer}:{rel.split('.')[0]}"
                if layer == "silver":
                    return f"silver:{rel.split('__')[0]}"
                return f"{layer}:{layer}"
            g = sorted({(group(a), group(b)) for a, b in edges if group(a) != group(b)})
            cnt = Counter(group(n) for n in nodes)
            lines = ["flowchart LR"]
            for n in sorted({x for e in g for x in e}):
                layer, rel = n.split(":", 1)
                lines.append(f'  {m_id(n)}["{layer}: {rel}<br/>{cnt[n]} object(s)"]')
            lines += [f"  {m_id(a)} --> {m_id(b)}" for a, b in g]
            return mermaid("\n".join(lines))
        lines = ["flowchart LR"]
        titles = {"source": "Source", "bronze": "Bronze", "silver": "Silver", "snapshot": "Silver history", "gold": "Gold"}
        by_layer: dict[str, list[str]] = defaultdict(list)
        for n in nodes:
            by_layer[n.split(":", 1)[0]].append(n)
        for layer in ["source", "bronze", "silver", "snapshot", "gold"] + sorted(set(by_layer) - set(titles)):
            if layer not in by_layer:
                continue
            lines.append(f'  subgraph {m_id("L_" + layer)}["{titles.get(layer, layer.title())}"]')
            shape = ('[("', '")]') if layer == "source" else ('[["', '"]]') if layer == "gold" else ('["', '"]')
            for n in by_layer[layer]:
                lines.append(f"    {m_id(n)}{shape[0]}{n.split(':', 1)[1]}{shape[1]}")
            lines.append("  end")
        lines += [f"  {m_id(a)} --> {m_id(b)}" for a, b in edges]
        return mermaid("\n".join(lines))

    def lineage_overview(self) -> None:
        p, here = self.p, "lineage/index.md"
        body = "Table-level lineage from the source through bronze and silver to gold. Column-level lineage is on [Column lineage (source-to-target mapping)](" + rel_link(here, "lineage/column-lineage.md") + ") and on every table page.\n\n"
        body += self._flow()
        rows = []
        for t in p.tables:
            silver = p.models.get(t.model)
            gold = sorted({u.name for u in p.consumers_of(t.model)}) if silver else []
            rows.append((self.link(here, self.source_page(t), t.qualified), code(t.bronze),
                         self.link(here, self.model_page(silver), silver.name) if silver else "—",
                         ", ".join(self.link(here, self.model_page(p.models[g]), g) for g in gold)))
        body += "\n" + table(["Source", "Bronze", "Silver", "Read by"], rows)
        body += "\n## How lineage is derived\n\n- **source → bronze:** the deployed plan's column mapping (`expr`, lossiness).\n- **bronze → silver → gold:** the dbt SQL parsed in Trino's dialect (sqlglot), `select *` expanded with upstream columns.\n- Edges marked _by name_ were matched on column names because the SQL did not parse.\n"
        if self.lin.fallbacks:
            body += f"\n{len(self.lin.fallbacks)} column(s) resolved by name:\n\n" + "\n".join(f"- `{x}`" for x in self.lin.fallbacks[:50]) + "\n"
        if self.lin.unresolved:
            body += "\n**Unresolved columns** (no upstream found):\n\n" + "\n".join(f"- `{x}`" for x in self.lin.unresolved) + "\n"
        self.add(here, "Lineage", 50, body)

    def column_lineage(self) -> None:
        p, here = self.p, "lineage/column-lineage.md"
        body = "Source-to-target mapping (STTM): one row per source column, through bronze and silver, with every gold column it reaches.\n\n"
        for t in p.tables:
            body += f"## {self.link(here, self.source_page(t), t.qualified)}\n\n"
            rows = []
            for c in t.columns:
                b = Node("bronze", t.bronze, c.name)
                s_edges = self.lin.down.get(b, [])
                silver = [(e.dst, e.transform) for e in s_edges if e.dst.layer == "silver"]
                golds = [n for n in self.lin.downstream(Node("source", t.qualified, c.name)) if n.layer == "gold"]
                rows.append((
                    code(c.name), code(c.pg_type), code(c.iceberg_type),
                    ("pass-through" if c.expr.strip('"') == c.name else code(c.expr)) + ("" if c.lossiness == "exact" else f" [{c.lossiness}]"),
                    "<br>".join(code(n.label) for n, _ in silver) or "—",
                    "<br>".join(tr if tr == "pass-through" else code(tr) for _, tr in silver) or "",
                    "<br>".join(code(n.label) for n in golds),
                ))
            body += table(["Source column", "Source type", "Bronze type", "Bronze mapping", "Silver column", "Silver transform", "Gold columns"], rows) + "\n"
        if p.gold:
            body += "## Gold columns back to source\n\n"
            rows = []
            for m in p.gold:
                for c in m.columns:
                    for n, path in self.lin.roots(Node("gold", m.name, c.name)) or [(None, [])]:
                        rows.append((code(f"{m.name}.{c.name}"), code(n.label) if n else "—", " → ".join(e.transform for e in path)))
            body += table(["Gold column", "Source column", "Transform chain (bronze → silver → gold)"], rows)
        self.add(here, "Column lineage (STTM)", 51, body)

    # ══ Quality ════════════════════════════════════════════════════════════════════════════════
    def quality_overview(self) -> None:
        p, here = self.p, "quality/index.md"
        e = p.evidence
        body = self.evidence_note()
        rows = []
        if e.reconcile is not None:
            c = Counter((r.get("verdict") or "").lower() for r in e.reconcile)
            missed = sum(1 for r in e.reconcile if r.get("control_fired") is False)
            rows.append((self.link(here, "quality/reconciliation.md", "Source ↔ bronze reconciliation"), f"{c.get('green', 0)} green, {c.get('diverged', 0)} diverged, {c.get('unverified', 0)} unverified", "✅" if c.get("green", 0) == len(e.reconcile) else "❌", f"planted controls missed: {missed}"))
        results = e.test_results()
        total = sum(len(m.all_tests()) for m in p.models.values())
        if results:
            failed = [k for k, r in results.items() if r.get("status") not in ("pass", "success")]
            rows.append((self.link(here, "quality/data-tests.md", "dbt data tests"), f"{len(results) - len(failed)}/{len(results)} passed", "✅" if not failed else "❌", f"{total} tests defined"))
        else:
            rows.append((self.link(here, "quality/data-tests.md", "dbt data tests"), f"{total} defined, no run results", "—", ""))
        if e.check is not None:
            failed = [x for x in e.check if not x.get("passed")]
            rows.append((self.link(here, "quality/code-quality.md", "Code quality (lakehouse check)"), f"{len(e.check) - len(failed)}/{len(e.check)} metrics met", "✅" if not failed else "❌", ", ".join(x.get("metric", "") for x in failed)))
        if e.status is not None:
            c = Counter(h.get("state") for h in e.status)
            rows.append((self.link(here, "quality/health.md", "Pipeline health"), ", ".join(f"{k}: {v}" for k, v in sorted(c.items())), "✅" if set(c) <= {"healthy"} else "⚠️", ""))
        body += table(["Area", "Result", "", "Notes"], rows)
        body += "\n## What is checked, and how\n\n" + table(["Check", "Layer", "How"], [
            ("Reconciliation", "source → bronze", "row counts, then bucketed canonical checksums (V1–V3), with a planted one-row control that must be detected"),
            ("Key uniqueness / not-null / relationships / accepted values", "silver", "dbt tests generated from source constraints, each naming its evidence"),
            ("Grain uniqueness, measure reconciliation", "gold", "dbt tests generated with the mart"),
            ("Lineage, test & doc coverage, drift, lint", "code", "`lakehouse check --lint`"),
            ("Freshness, failures, unverified runs, schema pending", "all", "`lakehouse status` from run facts"),
        ])
        body += "\n## Sign-off\n\n" + manual("quality-signoff", "_UAT / reconciliation sign-off: who verified, when, against which evidence run._")
        self.add(here, "Data quality & reconciliation", 60, body)

    def reconciliation(self) -> None:
        p, here = self.p, "quality/reconciliation.md"
        e = p.evidence.reconcile
        body = self.evidence_note()
        body += "Each table is compared between the source and bronze on every run. A tier that agrees proves nothing unless the planted control — one deliberately altered row — is detected; a missed control marks the result **unverified**.\n\n"
        if e is None:
            body += "_No reconciliation results collected._\n"
        else:
            rows = []
            for r in e:
                t = p.table(r.get("table", ""))
                tiers = "<br>".join(f"{x.get('tier')}: {'✅' if not x.get('divergences') else '❌ ' + '; '.join(x['divergences'][:3])}" for x in r.get("tiers", []))
                rows.append((self.link(here, self.source_page(t), r.get("table")) if t else r.get("table"), self.status_badge(r.get("verdict")), tiers,
                             {True: "fired", False: "❌ MISSED", None: "n/a (no rows)"}[r.get("control_fired")], ", ".join(r.get("not_compared", [])), r.get("control_detail", "")))
            body += table(["Table", "Verdict", "Tiers", "Control", "Not compared", "Control detail"], rows)
            missing = [t.qualified for t in p.tables if not self.recon(t.qualified)]
            if missing:
                body += f"\n⚠️ Not reconciled in this evidence: {', '.join(missing)}.\n"
        self.add(here, "Reconciliation results", 61, body)

    def data_tests(self) -> None:
        p, here = self.p, "quality/data-tests.md"
        body = self.evidence_note()
        body += "Every data-quality rule, the fact it comes from and its last result. Rules are generated: a primary key becomes `unique` + `not_null`, a NOT NULL column `not_null`, a declared foreign key `relationships`, an enum `accepted_values`; gold marts get grain uniqueness and measure reconciliation.\n\n"
        rule_count = Counter(t.rule or t.name for m in p.models.values() for t in m.all_tests())
        body += "## Rule catalogue\n\n" + table(["Rule", "Tests"], sorted(rule_count.items()))
        body += "\n## Coverage\n\n"
        cov = []
        for m in sorted(p.models.values(), key=lambda x: (x.layer, x.name)):
            tests = m.all_tests()
            kinds = {t.name for t in tests}
            cov.append((self.link(here, self.model_page(m), m.name), m.layer, len(tests),
                        "✅" if kinds & {"unique", "ekos_unique_combination"} else "—", "✅" if "not_null" in kinds else "—", "✅" if "relationships" in kinds else "—",
                        f"{sum(1 for c in m.columns if c.description)}/{len(m.columns)}"))
        body += table(["Model", "Layer", "Tests", "Key", "Not null", "Relationships", "Documented columns"], cov)
        results = p.evidence.test_results()
        failed = {k: r for k, r in results.items() if r.get("status") not in ("pass", "success")}
        if failed:
            body += "\n## Failing tests\n\n" + table(["Test", "Status", "Failing rows", "Message"], [(code(k), self.status_badge(r.get("status")), r.get("failures"), r.get("message")) for k, r in sorted(failed.items())])
        self.add(here, "Data quality tests", 62, body)

    def code_quality(self) -> None:
        here = "quality/code-quality.md"
        e = self.p.evidence.check
        body = self.evidence_note() + "`lakehouse check --lint` measures the generated project: lineage completeness (the dbt project compiled back into the EKOS ledger), test and documentation coverage, drift from generated code and `sqlfluff` lint.\n\n"
        if e is None:
            body += "_No check results collected._\n"
        else:
            body += table(["Metric", "Measured", "Passed", "Method", "Gaps"], [
                (x.get("metric"), f"{x.get('measured')}/{x.get('of')}", "✅" if x.get("passed") else "❌", x.get("method"),
                 "<br>".join((x.get("gaps") or [])[:15]) + (f"<br>… {len(x['gaps']) - 15} more" if len(x.get("gaps") or []) > 15 else ""))
                for x in e
            ])
        self.add(here, "Code quality", 63, body)

    def health_page(self) -> None:
        here = "quality/health.md"
        e = self.p.evidence.status
        body = self.evidence_note() + "One state per table and layer, with the facts that put it there: `healthy`, `late`, `unverified` (a control missed), `failing`, `schema_pending` (an open change proposal).\n\n"
        if e is None:
            body += "_No status collected._\n"
        else:
            def ts(v: int | None) -> str:
                return dt.datetime.fromtimestamp(v, dt.UTC).strftime("%Y-%m-%d %H:%M UTC") if v else "—"
            body += table(["Table", "Layer", "State", "Evidence", "Cause", "Last success", "Last run"], [
                (h.get("table"), h.get("layer"), self.status_badge(h.get("state")), "<br>".join(h.get("evidence") or []), h.get("cause") or "",
                 ts(h.get("last_success_at")), (h.get("last_run") or {}).get("run_id", ""))
                for h in sorted(e, key=lambda x: (x.get("layer", ""), x.get("table", "")))
            ])
        self.add(here, "Pipeline health", 64, body)

    # ══ Governance ═════════════════════════════════════════════════════════════════════════════
    def governance_overview(self) -> None:
        here = "governance/index.md"
        body = table(["Page", "Covers"], [
            (self.link(here, "governance/data-dictionary.md", "Data dictionary"), "every silver and gold column: meaning, type, source, PII"),
            (self.link(here, "governance/pii-and-security.md", "PII and security"), "classified columns, masking, where personal data flows, credentials"),
        ])
        body += "\n## Policies\n\n" + manual("governance-policies", "_Classification scheme, retention, access approval process, applicable regulation (e.g. GDPR / ZZPL)._")
        self.add(here, "Governance", 70, body)

    def data_dictionary(self) -> None:
        p, here = self.p, "governance/data-dictionary.md"
        masked = self._masked()
        body = "Business-facing columns: silver (conformed entities) and gold (marts). Descriptions come from source comments and mart definitions.\n\n"
        for m in sorted(p.gold, key=lambda x: x.name) + sorted(p.silver, key=lambda x: x.name):
            t = p.table_by_model(m.name)
            body += f"## {self.link(here, self.model_page(m), m.name)} ({m.layer})\n\n"
            rows = []
            for c in m.columns:
                srcs = [n.label for n, _ in self.lin.roots(Node(m.layer, m.name, c.name))]
                sc = t.column(c.name) if t else None
                pii = []
                for s in srcs:
                    st, sn = s.rsplit(".", 1)
                    tt = p.table(st)
                    col = tt.column(sn) if tt else None
                    if col and col.pii_candidate:
                        pii.append(col.pii_candidate + (f" ({masked[s]})" if s in masked else " ⚠️ unmasked"))
                rows.append((code(c.name), code(c.data_type or (sc.iceberg_type if sc else "")), c.description or (sc.comment if sc and sc.comment else ""),
                             "required" if sc and sc.not_null else "", ", ".join(srcs), ", ".join(sorted(set(pii)))))
            body += table(["Column", "Type", "Description", "Nullability", "Source", "PII"], rows) + "\n"
        self.add(here, "Data dictionary", 71, body)

    def pii(self) -> None:
        p, here = self.p, "governance/pii-and-security.md"
        masked = self._masked()
        rows = []
        for t in p.tables:
            for c in t.columns:
                key = f"{t.qualified}.{c.name}"
                if not c.pii_candidate and key not in masked:
                    continue
                reach = [n for n in self.lin.downstream(Node("source", t.qualified, c.name)) if n.layer != "bronze"]
                rows.append((code(key), c.pii_candidate or "", masked.get(key, "⚠️ not masked"), "<br>".join(code(n.label) for n in reach)))
        body = "PII candidates come from EKOS's name classifier (RFC 0157) and are **reported, never applied**. A person approves masking per column in `[extensions.lakehouse.pii]`: `null` keeps the column without values, `hmac` replaces each value with HMAC-SHA256 under `EKOS_LH_PII_KEY` (from the environment; compiled SQL under `dbt/target/` is git-ignored).\n\n"
        body += "> Bronze keeps source values unmasked. Restrict bronze namespaces to the pipeline's service identities.\n\n"
        body += table(["Column", "Classifier", "Silver masking", "Reaches"], rows)
        body += "\n## Credentials\n\nConfiguration names environment variables, never secrets:\n\n" + table(["Secret", "Where it is read"], [
            ("Source password", "`source-password-env` in `[extensions.lakehouse]`"),
            ("Masking key", "`EKOS_LH_PII_KEY`"),
            ("Alert webhook", "`alert-webhook-env`"),
        ])
        body += "\n## Access\n\n" + manual("pii-access", "_Who can read bronze, silver and gold; how access is requested and reviewed; data retention per layer._")
        self.add(here, "PII and security", 72, body)

    # ══ Operations ═════════════════════════════════════════════════════════════════════════════
    def operations_overview(self) -> None:
        here = "operations/index.md"
        body = table(["Page", "Covers"], [
            (self.link(here, "operations/runbook.md", "Runbook"), "schedules, commands, what to do per health state"),
            (self.link(here, "operations/sla.md", "SLAs and data contracts"), "freshness, availability, per-table contracts"),
            (self.link(here, "operations/schema-changes.md", "Schema changes"), "open proposals and applied changes"),
            (self.link(here, "operations/changelog.md", "Change log"), "every commit to the pipeline repository"),
        ])
        self.add(here, "Operations", 80, body)

    def runbook(self) -> None:
        s, here = self.p.settings, "operations/runbook.md"
        body = "## Schedules\n\n" + table(["Job", "Cron"], [("Bronze loads", code(s.get("bronze_schedule"))), ("Iceberg maintenance", code(s.get("maintenance_schedule")))])
        body += "\n## Everyday commands\n\n```bash\n" + "\n".join([
            "ekos-lakehouse lakehouse doctor            # which services answer",
            "ekos-lakehouse lakehouse status            # health per table and layer",
            "ekos-lakehouse lakehouse reconcile         # V1–V3 with a planted control",
            "ekos-lakehouse lakehouse check --lint      # lineage, coverage, drift, lint",
            "ekos-lakehouse lakehouse sync              # apply safe source changes, propose the rest",
            "ekos-lakehouse lakehouse proposals         # what waits for a merge",
            "ekos-lakehouse lakehouse reload --table <schema.table>   # full reload on the next run",
        ]) + "\n```\n"
        body += "\n## By health state\n\n" + table(["State", "Meaning", "First action"], [
            ("late", "no successful run within `late-after-minutes`", "check Dagster for a stuck or failed run; `lakehouse doctor`"),
            ("failing", "the last run or a test failed", "read the cause in `lakehouse status`; failing silver tests link to the source change or data finding"),
            ("unverified", "reconciliation agreed but the planted control was missed", "rerun `lakehouse reconcile --table …`; do not trust the table until green"),
            ("schema_pending", "a breaking source change waits on a proposal", "review the branch `ekos/change/<table>/<id>`; merging is the approval"),
            ("diverged (reconciliation)", "bronze differs from the source", "check watermark validity; `lakehouse reload --table …` if the watermark missed updates"),
        ])
        body += "\n## Contacts and escalation\n\n" + manual("runbook-contacts", "_On-call rotation, escalation path, source-system DBA contact._")
        body += "\n## Known issues\n\n" + manual("runbook-known-issues", "_Recurring incidents and their fixes._")
        self.add(here, "Runbook", 81, body)

    def sla(self) -> None:
        p, cfg, here = self.p, self.cfg, "operations/sla.md"
        body = "## Service levels\n\n"
        if cfg.sla:
            body += table(["Measure", "Target"], sorted(cfg.sla.items()))
        body += manual("sla", "_Freshness targets per layer, availability window, support hours, RPO/RTO (Iceberg snapshots retain history until expiry)._")
        body += "\n## Data contracts (generated)\n\nWhat each silver table guarantees to its consumers, derived from the source constraints it is tested against. A change that would break a contract arrives as a proposal, not as a silent change.\n\n"
        for m in sorted(p.silver, key=lambda x: x.name):
            t = p.table_by_model(m.name)
            if not t:
                continue
            nn = [c.name for c in m.columns if any(x.name == "not_null" for x in c.tests)]
            fk = [x.describe() for c in m.columns for x in c.tests if x.name == "relationships"]
            av = [f"{c.name} ∈ {x.args.get('values')}" for c in m.columns for x in c.tests if x.name == "accepted_values"]
            body += f"### {self.link(here, self.model_page(m), m.name)}\n\n```yaml\n"
            body += f"relation: {m.relation}\nsource: {t.qualified}\ngrain: {t.primary_key or 'every source row'}\nrefresh: {p.settings.get('bronze_schedule')} ({t.mode})\n"
            body += f"not_null: {nn}\n" + (f"references: {fk}\n" if fk else "") + (f"accepted_values: {av}\n" if av else "")
            body += "schema_change_policy: additive changes automatic; breaking changes by approved proposal\n```\n\n"
        self.add(here, "SLAs and data contracts", 82, body)

    def schema_changes(self) -> None:
        p, here = self.p, "operations/schema-changes.md"
        body = "Source changes are classified each run. Safe classes are committed; breaking classes become proposals with their impact (the silver columns and tests they reach).\n\n"
        open_b = (p.evidence.proposals or {}).get("proposals") or []
        body += "## Waiting for a merge\n\n" + (table(["Kind", "Subject", "Summary", "Branch"], [(b.get("kind"), b.get("subject"), b.get("summary"), code(b.get("branch"))) for b in open_b]) if p.evidence.proposals is not None else "_Not collected._\n")
        sc = [x for x in p.proposals if x.kind == "schema change"]
        if sc:
            body += "\n## Proposals in the repository\n\n"
            for x in sc:
                body += f"### {esc(x.title)}\n\n" + re.sub(r"^# .*\n", "", x.body).strip() + "\n\n"
        syncs = [c for c in p.commits if "sync" in c.subject.lower() or "change" in c.subject.lower() or "rename" in c.subject.lower()]
        body += "\n## Applied changes (git history)\n\n" + table(["Date", "Commit", "Change"], [(c.date, code(c.sha), c.subject) for c in syncs])
        body += "\n## Policy\n\n" + table(["Change in source", "Default action"], [
            ("New table", "auto: bronze + silver generated"), ("New nullable column", "auto: Iceberg ADD COLUMN"),
            ("Safe widening (int → bigint, …)", "auto: type promotion"), ("New NOT NULL column / default", "proposal: backfill decision"),
            ("Column rename", "proposal: rename, never drop + add"), ("Column or table drop", "proposal with impact"),
            ("Narrowing / incompatible type", "proposal; table paused"), ("Key change", "proposal; silver dedup paused"),
            ("Watermark trigger changed", "proposal"),
        ]) + "\nOverrides live in `[extensions.lakehouse.policy]`.\n"
        self.add(here, "Schema changes", 83, body)

    def changelog(self) -> None:
        p, here = self.p, "operations/changelog.md"
        body = "Every commit to the pipeline repository — generated changes, approved proposals and hand edits.\n\n"
        body += table(["Date", "Commit", "Author", "Subject"], [(c.date, code(c.sha), c.author, c.subject) for c in p.commits]) if p.commits else "_The pipeline directory is not a git repository._\n"
        body += "\n## Release notes\n\n" + manual("release-notes", "_Human-readable notes per release: what changed for consumers._")
        self.add(here, "Change log", 84, body)

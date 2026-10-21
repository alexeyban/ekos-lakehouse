"""Load everything the documentation is built from into one in-memory model.

Sources, in order of authority:

1. ``lakehouse.lock.json`` — the deployed plan: source tables, columns, keys, foreign keys,
   comments, PII candidates, the PostgreSQL → Iceberg mapping and each table's load mode.
2. The generated dbt project under ``dbt/`` — silver and gold models, their YAML (tests with
   ``meta.ekos_rule`` / ``evidence``), snapshots. ``dbt/target/manifest.json`` is used when present.
3. ``marts/*.md`` — gold mart descriptions written by stakeholders.
4. ``proposals/*.md`` — schema-change and gold proposals.
5. The evidence directory — JSON saved from ``ekos-lakehouse lakehouse … --json`` and dbt's
   ``run_results.json`` (see ``collect.py``).
6. ``git log`` of the pipeline repository — the change history.
"""

from __future__ import annotations

import json
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml


# ── Source / bronze (from the lock) ────────────────────────────────────────────────────────────


@dataclass
class Column:
    name: str
    pg_type: str
    not_null: bool = False
    has_default: bool = False
    comment: str | None = None
    enum_labels: list[str] = field(default_factory=list)
    pii_candidate: str | None = None
    attnum: int = 0
    # bronze mapping
    iceberg_type: str = ""
    trino_type: str | None = None
    expr: str = ""
    lossiness: str = "exact"
    rule: str = ""
    rationale: str = ""
    compared: bool = True


@dataclass
class ForeignKey:
    name: str
    columns: list[str]
    ref_schema: str
    ref_table: str
    ref_columns: list[str]

    @property
    def ref(self) -> str:
        return f"{self.ref_schema}.{self.ref_table}"


@dataclass
class SourceTable:
    schema: str
    table: str
    oid: int
    namespace: str
    primary_key: list[str]
    columns: list[Column]
    foreign_keys: list[ForeignKey]
    comment: str | None
    mode: str                      # "full" | "incremental"
    watermark: str | None
    keyset_deletes: bool
    mode_evidence: list[str]
    mode_notes: list[str]
    renames: list[dict[str, str]]

    @property
    def qualified(self) -> str:
        return f"{self.schema}.{self.table}"

    @property
    def bronze(self) -> str:
        return f"{self.namespace}.{self.table}"

    @property
    def model(self) -> str:
        return f"{self.schema}__{self.table}"

    def column(self, name: str) -> Column | None:
        return next((c for c in self.columns if c.name == name), None)

    def fk_for(self, column: str) -> ForeignKey | None:
        return next((f for f in self.foreign_keys if column in f.columns), None)


@dataclass
class Refused:
    qualified: str
    reasons: list[str]


# ── dbt (silver / gold) ────────────────────────────────────────────────────────────────────────


@dataclass
class Test:
    name: str                 # dbt test type: unique, not_null, relationships, accepted_values, …
    column: str | None
    rule: str | None          # meta.ekos_rule
    evidence: str | None      # meta.evidence
    args: dict[str, Any] = field(default_factory=dict)

    def describe(self) -> str:
        if self.name == "relationships":
            to = str(self.args.get("to", ""))
            m = re.search(r"ref\('([^']+)'\)", to)
            return f"relationships → {m.group(1) if m else to}.{self.args.get('field', '')}"
        if self.name == "accepted_values":
            return f"accepted_values {self.args.get('values', [])}"
        if self.name == "ekos_unique_combination":
            cols = [str(c).strip('"') for c in self.args.get("columns", [])]
            return f"unique combination ({', '.join(cols)})"
        return self.name


@dataclass
class ModelColumn:
    name: str
    data_type: str = ""
    description: str = ""
    tests: list[Test] = field(default_factory=list)


@dataclass
class Model:
    name: str
    layer: str                        # silver | gold | pentaho | other
    path: str                         # relative to the pipeline repo
    sql: str
    description: str = ""
    columns: list[ModelColumn] = field(default_factory=list)
    tests: list[Test] = field(default_factory=list)    # model-level
    meta: dict[str, Any] = field(default_factory=dict)
    refs: list[str] = field(default_factory=list)
    sources: list[tuple[str, str]] = field(default_factory=list)
    schema: str = ""
    alias: str = ""
    materialized: str = "table"

    @property
    def relation(self) -> str:
        return f"{self.schema}.{self.alias or self.name}" if self.schema else self.name

    def column(self, name: str) -> ModelColumn | None:
        return next((c for c in self.columns if c.name == name), None)

    def all_tests(self) -> list[Test]:
        return self.tests + [t for c in self.columns for t in c.tests]


@dataclass
class Snapshot:
    name: str
    relation: str
    strategy: str
    unique_key: Any
    alias: str


@dataclass
class Mart:
    mart: str
    path: str
    grain: list[str]
    dimensions: dict[str, str]
    measures: dict[str, str]
    filters: list[str]
    prose: str
    questions: str | None = None      # marts/<mart>.questions.md, when the draft asked back


@dataclass
class Proposal:
    id: str
    path: str
    kind: str                         # "gold mart" | "schema change"
    title: str
    body: str


@dataclass
class Commit:
    sha: str
    date: str
    author: str
    subject: str


@dataclass
class Evidence:
    collected_at: str | None = None
    status: list[dict[str, Any]] | None = None
    check: list[dict[str, Any]] | None = None
    reconcile: list[dict[str, Any]] | None = None
    tables: dict[str, Any] | None = None
    proposals: dict[str, Any] | None = None
    run_results: dict[str, Any] | None = None
    sync: dict[str, Any] | None = None
    meta: dict[str, Any] = field(default_factory=dict)

    def test_results(self) -> dict[str, dict[str, Any]]:
        """dbt test results by test node name (the part after ``test.<project>.``)."""
        out: dict[str, dict[str, Any]] = {}
        for r in (self.run_results or {}).get("results", []):
            uid = r.get("unique_id", "")
            if not uid.startswith("test."):
                continue
            name = uid.split(".")[2] if uid.count(".") >= 2 else uid
            out[name] = r
        return out

    def model_results(self) -> dict[str, dict[str, Any]]:
        out: dict[str, dict[str, Any]] = {}
        for r in (self.run_results or {}).get("results", []):
            uid = r.get("unique_id", "")
            if uid.startswith(("model.", "snapshot.")):
                out[uid.split(".")[-1]] = r
        return out


@dataclass
class Project:
    root: Path
    settings: dict[str, Any]
    tables: list[SourceTable]
    refused: list[Refused]
    models: dict[str, Model]
    snapshots: list[Snapshot]
    marts: list[Mart]
    proposals: list[Proposal]
    commits: list[Commit]
    evidence: Evidence
    head: str | None
    lock_format: int

    def table(self, qualified: str) -> SourceTable | None:
        return next((t for t in self.tables if t.qualified == qualified), None)

    def table_by_model(self, model: str) -> SourceTable | None:
        return next((t for t in self.tables if t.model == model), None)

    @property
    def silver(self) -> list[Model]:
        return [m for m in self.models.values() if m.layer == "silver"]

    @property
    def gold(self) -> list[Model]:
        return [m for m in self.models.values() if m.layer == "gold"]

    @property
    def other_models(self) -> list[Model]:
        return [m for m in self.models.values() if m.layer not in ("silver", "gold")]

    def consumers_of(self, model: str) -> list[Model]:
        return [m for m in self.models.values() if model in m.refs]


# ── Loading ────────────────────────────────────────────────────────────────────────────────────


def _load_lock(root: Path) -> tuple[dict[str, Any], list[SourceTable], list[Refused], int]:
    lock_path = root / "lakehouse.lock.json"
    if not lock_path.exists():
        raise FileNotFoundError(
            f"{lock_path} not found — run `ekos-lakehouse lakehouse generate` first, or pass "
            "--pipeline pointing at the pipeline repository"
        )
    lock = json.loads(lock_path.read_text(encoding="utf-8"))
    tables: list[SourceTable] = []
    for t in lock.get("tables", []):
        facts = t.get("facts", {})
        fcols = {c["name"]: c for c in facts.get("columns", [])}
        cols: list[Column] = []
        for m in t.get("columns", []):
            f = fcols.get(m["column"], {})
            cols.append(
                Column(
                    name=m["column"],
                    pg_type=m.get("pg_type", ""),
                    not_null=f.get("not_null", False),
                    has_default=f.get("has_default", False),
                    comment=f.get("comment"),
                    enum_labels=f.get("enum_labels", []),
                    pii_candidate=f.get("pii_candidate"),
                    attnum=f.get("attnum", 0),
                    iceberg_type=m.get("iceberg_type", ""),
                    trino_type=m.get("trino_type"),
                    expr=m.get("expr", ""),
                    lossiness=m.get("lossiness", "exact"),
                    rule=m.get("rule", ""),
                    rationale=m.get("rationale", ""),
                    compared=m.get("compare") is not None,
                )
            )
        decision = t.get("decision", {})
        mode = decision.get("mode", {"mode": "full"})
        tables.append(
            SourceTable(
                schema=t["schema"],
                table=t["table"],
                oid=t.get("oid", 0),
                namespace=t.get("namespace", ""),
                primary_key=t.get("primary_key", []),
                columns=cols,
                foreign_keys=[ForeignKey(**fk) for fk in facts.get("foreign_keys", [])],
                comment=facts.get("comment"),
                mode=mode.get("mode", "full"),
                watermark=mode.get("watermark"),
                keyset_deletes=bool(mode.get("keyset_deletes", False)),
                mode_evidence=decision.get("evidence", []),
                mode_notes=decision.get("notes", []),
                renames=t.get("renames", []),
            )
        )
    refused = [
        Refused(f"{r['schema']}.{r['table']}", r.get("reasons", [])) for r in lock.get("refused", [])
    ]
    # The lock writes settings in kebab-case (`source-catalog`, `bronze-schedule`); pages read them
    # as Python names.
    settings = {k.replace("-", "_"): v for k, v in (lock.get("settings") or {}).items()}
    return settings, tables, refused, lock.get("format", 0)


_REF = re.compile(r"\{\{\s*ref\(\s*['\"]([^'\"]+)['\"]\s*\)\s*\}\}")
_SOURCE = re.compile(r"\{\{\s*source\(\s*['\"]([^'\"]+)['\"]\s*,\s*['\"]([^'\"]+)['\"]\s*\)\s*\}\}")
_CONFIG = re.compile(r"\{\{\s*config\((.*?)\)\s*\}\}", re.S)


def _config_kw(sql: str) -> dict[str, str]:
    m = _CONFIG.search(sql)
    if not m:
        return {}
    return dict(re.findall(r"(\w+)\s*=\s*['\"]([^'\"]*)['\"]", m.group(1)))


def _tests(entries: list[Any] | None, column: str | None) -> list[Test]:
    out: list[Test] = []
    for e in entries or []:
        if isinstance(e, str):
            out.append(Test(e, column, None, None))
            continue
        if not isinstance(e, dict) or not e:
            continue
        name, body = next(iter(e.items()))
        body = body or {}
        meta = (body.get("config") or {}).get("meta") or body.get("meta") or {}
        args = dict(body.get("arguments") or {})
        args.update({k: v for k, v in body.items() if k not in ("config", "arguments", "meta")})
        out.append(Test(name, column, meta.get("ekos_rule"), meta.get("evidence"), args))
    return out


def _layer(path: Path) -> str:
    parts = path.parts
    for layer in ("silver", "gold"):
        if layer in parts:
            return layer
    return "pentaho" if "pentaho" in "/".join(parts) else "other"


def _load_dbt(root: Path) -> tuple[dict[str, Model], list[Snapshot]]:
    dbt = root / "dbt"
    models: dict[str, Model] = {}
    if not (dbt / "models").exists():
        return models, []
    for sql_path in sorted((dbt / "models").rglob("*.sql")):
        sql = sql_path.read_text(encoding="utf-8")
        rel = sql_path.relative_to(root)
        cfg = _config_kw(sql)
        models[sql_path.stem] = Model(
            name=sql_path.stem,
            layer=_layer(sql_path.relative_to(dbt / "models")),
            path=str(rel),
            sql=sql,
            refs=sorted(set(_REF.findall(sql))),
            sources=sorted(set(_SOURCE.findall(sql))),
            schema=cfg.get("schema", ""),
            alias=cfg.get("alias", ""),
            materialized=cfg.get("materialized", "table"),
        )
    snapshots: list[Snapshot] = []
    ymls = list((dbt / "models").rglob("*.yml"))
    if (dbt / "snapshots").exists():
        ymls += list((dbt / "snapshots").rglob("*.yml"))
    for yml in sorted(ymls):
        doc = yaml.safe_load(yml.read_text(encoding="utf-8")) or {}
        for m in doc.get("models", []) or []:
            model = models.get(m.get("name"))
            if model is None:
                continue
            model.description = (m.get("description") or "").strip()
            model.meta = ((m.get("config") or {}).get("meta") or m.get("meta") or {})
            model.tests = _tests(m.get("data_tests") or m.get("tests"), None)
            model.columns = [
                ModelColumn(
                    name=c["name"],
                    data_type=c.get("data_type", "") or "",
                    description=(c.get("description") or "").strip(),
                    tests=_tests(c.get("data_tests") or c.get("tests"), c["name"]),
                )
                for c in m.get("columns", []) or []
            ]
        for s in doc.get("snapshots", []) or []:
            cfg = s.get("config", {}) or {}
            snapshots.append(
                Snapshot(
                    name=s["name"],
                    relation=str(s.get("relation", "")),
                    strategy=cfg.get("strategy", ""),
                    unique_key=cfg.get("unique_key"),
                    alias=cfg.get("alias", ""),
                )
            )
    _enrich_from_manifest(dbt, models)
    return models, snapshots


def _enrich_from_manifest(dbt: Path, models: dict[str, Model]) -> None:
    """With ``dbt/target/manifest.json`` (after ``dbt parse``/``build``) the column list, data
    types and descriptions come from dbt itself; with ``catalog.json`` (``dbt docs generate``)
    the warehouse's real column types fill any gaps."""
    manifest = dbt / "target" / "manifest.json"
    catalog = dbt / "target" / "catalog.json"
    if manifest.exists():
        nodes = json.loads(manifest.read_text(encoding="utf-8")).get("nodes", {})
        for node in nodes.values():
            if node.get("resource_type") != "model" or node.get("name") not in models:
                continue
            m = models[node["name"]]
            m.description = m.description or (node.get("description") or "")
            m.schema = m.schema or node.get("schema", "")
            m.alias = m.alias or node.get("alias", "")
            have = {c.name for c in m.columns}
            for name, c in (node.get("columns") or {}).items():
                if name not in have:
                    m.columns.append(ModelColumn(name, c.get("data_type") or "", c.get("description") or ""))
    if catalog.exists():
        cat = json.loads(catalog.read_text(encoding="utf-8")).get("nodes", {})
        for uid, node in cat.items():
            name = uid.split(".")[-1]
            m = models.get(name)
            if not m:
                continue
            cols = node.get("columns", {})
            have = {c.name: c for c in m.columns}
            for cname, c in sorted(cols.items(), key=lambda kv: kv[1].get("index", 0)):
                if cname in have:
                    have[cname].data_type = have[cname].data_type or c.get("type", "")
                else:
                    m.columns.append(ModelColumn(cname, c.get("type", "")))


def _gold_columns_from_sql(models: dict[str, Model]) -> None:
    """A gold model without YAML columns still lists the names its SELECT produces."""
    for m in models.values():
        if m.columns:
            continue
        for alias in re.findall(r"\bas\s+\"?([A-Za-z_][A-Za-z0-9_]*)\"?\s*(?:,|\n\s*from)", m.sql, re.I):
            m.columns.append(ModelColumn(alias))


_FRONT = re.compile(r"^---\s*\n(.*?)\n---\s*\n?(.*)$", re.S)


def _load_marts(root: Path) -> list[Mart]:
    out: list[Mart] = []
    d = root / "marts"
    if not d.exists():
        return out
    for p in sorted(d.glob("*.md")):
        if p.name.endswith(".questions.md"):
            continue
        text = p.read_text(encoding="utf-8")
        m = _FRONT.match(text)
        header = yaml.safe_load(m.group(1)) if m else {}
        prose = (m.group(2) if m else text).strip()
        name = header.get("mart", p.stem)
        q = p.with_suffix(".questions.md")
        out.append(
            Mart(
                mart=name,
                path=str(p.relative_to(root)),
                grain=list(header.get("grain", []) or []),
                dimensions={str(k): str(v) for k, v in (header.get("dimensions") or {}).items()},
                measures={str(k): str(v) for k, v in (header.get("measures") or {}).items()},
                filters=[str(f) for f in header.get("filters", []) or []],
                prose=prose,
                questions=q.read_text(encoding="utf-8") if q.exists() else None,
            )
        )
    return out


def _load_proposals(root: Path) -> list[Proposal]:
    out: list[Proposal] = []
    d = root / "proposals"
    if not d.exists():
        return out
    for p in sorted(d.glob("*.md")):
        body = p.read_text(encoding="utf-8")
        title = next((ln.lstrip("# ").strip() for ln in body.splitlines() if ln.startswith("#")), p.stem)
        kind = "gold mart" if p.stem.startswith("gold-") else "schema change"
        out.append(Proposal(p.stem, str(p.relative_to(root)), kind, title, body))
    return out


def _git(root: Path, *args: str) -> str | None:
    try:
        return subprocess.run(
            ["git", "-C", str(root), *args], capture_output=True, text=True, check=True
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return None


def _load_commits(root: Path, limit: int = 200) -> tuple[list[Commit], str | None]:
    out = _git(root, "log", f"-{limit}", "--date=short", "--pretty=format:%h\x1f%ad\x1f%an\x1f%s")
    commits = []
    for line in (out or "").splitlines():
        parts = line.split("\x1f")
        if len(parts) == 4:
            commits.append(Commit(*parts))
    head = (_git(root, "rev-parse", "--short", "HEAD") or "").strip() or None
    return commits, head


def _read_json(p: Path) -> Any:
    if not p.exists():
        return None
    text = p.read_text(encoding="utf-8").strip()
    return json.loads(text) if text else None


def load_evidence(d: Path | None) -> Evidence:
    ev = Evidence()
    if d is None or not d.exists():
        return ev
    ev.status = _read_json(d / "status.json")
    ev.check = _read_json(d / "check.json")
    ev.reconcile = _read_json(d / "reconcile.json")
    ev.tables = _read_json(d / "tables.json")
    ev.proposals = _read_json(d / "proposals.json")
    ev.sync = _read_json(d / "sync.json")
    ev.run_results = _read_json(d / "run_results.json")
    ev.meta = _read_json(d / "collected.json") or {}
    ev.collected_at = ev.meta.get("collected_at")
    return ev


def load(root: Path, evidence_dir: Path | None) -> Project:
    root = root.resolve()
    settings, tables, refused, fmt = _load_lock(root)
    models, snapshots = _load_dbt(root)
    _gold_columns_from_sql(models)
    commits, head = _load_commits(root)
    if evidence_dir is None:
        evidence_dir = root / "docs" / "_evidence"
    ev = load_evidence(evidence_dir)
    if ev.run_results is None:
        ev.run_results = _read_json(root / "dbt" / "target" / "run_results.json")
    return Project(
        root=root,
        settings=settings,
        tables=tables,
        refused=refused,
        models=models,
        snapshots=snapshots,
        marts=_load_marts(root),
        proposals=_load_proposals(root),
        commits=commits,
        evidence=ev,
        head=head,
        lock_format=fmt,
    )

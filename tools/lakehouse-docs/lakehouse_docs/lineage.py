"""Column-level lineage: source → bronze → silver → gold (and any other dbt model).

* source → bronze comes from the plan: every bronze column is the mapping's ``expr`` over one
  source column, with its lossiness class.
* bronze → silver and model → model come from the SQL itself, parsed with sqlglot in Trino's
  dialect after the dbt Jinja is resolved (``source()``/``ref()`` → relations, ``var()`` → a
  literal). ``select *`` is expanded with the upstream schema, so the generated silver shape
  (CTE over ``select *``) resolves to real columns.
* When a statement does not parse, the model falls back to matching column names with its
  upstream relations and the edge is marked ``inferred``.
"""

from __future__ import annotations

import re
from collections import defaultdict
from dataclasses import dataclass

from .project import Model, Project

BRONZE_META = ["_ekos_loaded_at", "_ekos_run_id", "_ekos_source_lsn", "_ekos_op"]


@dataclass(frozen=True)
class Node:
    layer: str      # source | bronze | silver | gold | <other layer>
    relation: str   # public.film | bronze_public.film | public__film | gold__x
    column: str

    @property
    def label(self) -> str:
        return f"{self.relation}.{self.column}"


@dataclass(frozen=True)
class Edge:
    src: Node
    dst: Node
    transform: str      # "pass-through" or the expression
    kind: str           # mapped | parsed | inferred


class Lineage:
    def __init__(self) -> None:
        self.edges: list[Edge] = []
        self.up: dict[Node, list[Edge]] = defaultdict(list)
        self.down: dict[Node, list[Edge]] = defaultdict(list)
        self.unresolved: list[str] = []
        self.fallbacks: list[str] = []   # columns resolved by name because the SQL did not parse

    def add(self, e: Edge) -> None:
        if e in self.up[e.dst]:
            return
        self.edges.append(e)
        self.up[e.dst].append(e)
        self.down[e.src].append(e)

    def roots(self, n: Node) -> list[tuple[Node, list[Edge]]]:
        """Every source-layer column ``n`` derives from, with the path of edges to it."""
        out: list[tuple[Node, list[Edge]]] = []
        stack: list[tuple[Node, list[Edge]]] = [(n, [])]
        seen: set[Node] = set()
        while stack:
            cur, path = stack.pop()
            if cur in seen:
                continue
            seen.add(cur)
            ups = self.up.get(cur, [])
            if not ups:
                if cur.layer == "source":
                    out.append((cur, list(reversed(path))))
                continue
            for e in ups:
                stack.append((e.src, path + [e]))
        return sorted(out, key=lambda x: x[0].label)

    def leaves(self, n: Node) -> list[Node]:
        """Every column downstream of ``n`` that nothing reads further."""
        out: list[Node] = []
        stack, seen = [n], set()
        while stack:
            cur = stack.pop()
            if cur in seen:
                continue
            seen.add(cur)
            downs = self.down.get(cur, [])
            if not downs and cur != n:
                out.append(cur)
            stack.extend(e.dst for e in downs)
        return sorted(out, key=lambda x: (x.layer, x.label))

    def downstream(self, n: Node) -> list[Node]:
        out: list[Node] = []
        stack, seen = [n], set()
        while stack:
            cur = stack.pop()
            if cur in seen:
                continue
            seen.add(cur)
            for e in self.down.get(cur, []):
                out.append(e.dst)
                stack.append(e.dst)
        return sorted(set(out), key=lambda x: (x.layer, x.label))


def _is_plain(expr: str, column: str) -> bool:
    e = expr.strip().strip('"')
    return e == column or re.fullmatch(rf'(\w+\.)?"?{re.escape(column)}"?', expr.strip()) is not None


def _resolve_jinja(sql: str) -> str:
    sql = re.sub(r"\{\{\s*config\(.*?\)\s*\}\}", "", sql, flags=re.S)
    sql = re.sub(r"\{#.*?#\}", "", sql, flags=re.S)
    sql = re.sub(
        r"\{\{\s*source\(\s*['\"]([^'\"]+)['\"]\s*,\s*['\"]([^'\"]+)['\"]\s*\)\s*\}\}",
        r'"\1"."\2"',
        sql,
    )
    sql = re.sub(r"\{\{\s*ref\(\s*['\"]([^'\"]+)['\"]\s*\)\s*\}\}", r'"__ref__"."\1"', sql)
    sql = re.sub(r"'\{\{.*?\}\}'", "'jinja'", sql, flags=re.S)
    sql = re.sub(r"\{\{.*?\}\}", "'jinja'", sql, flags=re.S)
    sql = re.sub(r"\{%.*?%\}", "", sql, flags=re.S)
    return sql


def build(p: Project) -> Lineage:
    lin = Lineage()
    bronze_cols: dict[str, list[str]] = {}
    for t in p.tables:
        for c in t.columns:
            src = Node("source", t.qualified, c.name)
            dst = Node("bronze", t.bronze, c.name)
            transform = "pass-through" if _is_plain(c.expr, c.name) else c.expr
            if c.lossiness != "exact":
                transform = f"{transform} [{c.lossiness}]"
            lin.add(Edge(src, dst, transform, "mapped"))
        bronze_cols[t.bronze] = [c.name for c in t.columns] + BRONZE_META

    model_cols: dict[str, list[str]] = {m.name: [c.name for c in m.columns] for m in p.models.values()}
    layer_of = {m.name: m.layer for m in p.models.values()}

    try:
        from sqlglot import exp
        from sqlglot.lineage import lineage as sg_lineage
    except ImportError:  # pragma: no cover - sqlglot is in requirements
        sg_lineage = None

    # Process models upstream first, so a model reading another uses its resolved columns.
    order = _topo(p.models)
    for name in order:
        m = p.models[name]
        # "db.table", lower-cased → (layer, relation id). sqlglot's Trino dialect folds identifiers
        # to lower case, quoted ones too, so Oracle's upper-case names are matched without case.
        upstream: dict[str, tuple[str, str]] = {}
        sch: dict[str, dict[str, dict[str, str]]] = {}
        for src_schema, src_table in m.sources:
            rel = f"{src_schema}.{src_table}"
            sch.setdefault(src_schema, {})[src_table] = {c: "varchar" for c in bronze_cols.get(rel, [])}
            upstream[rel.lower()] = ("bronze", rel)
        for r in m.refs:
            sch.setdefault("__ref__", {})[r] = {c: "varchar" for c in model_cols.get(r, [])}
            upstream[f"__ref__.{r}".lower()] = (layer_of.get(r, "other"), r)
        sql = _resolve_jinja(m.sql)
        out_cols = [c.name for c in m.columns]
        for col in out_cols:
            dst = Node(m.layer, m.name, col)
            done = False
            parsed_expr: str | None = None   # set when the SQL parsed but read no column (a constant)
            if sg_lineage is not None:
                try:
                    node = sg_lineage(col, sql, schema=sch, dialect="trino")
                    expr_sql = node.expression.sql(dialect="trino")
                    expr_sql = re.sub(r'\s+AS\s+"?[\w]+"?\s*$', "", expr_sql, flags=re.I)
                    leaves = [n for n in node.walk() if not n.downstream]
                    parsed_expr = expr_sql
                    for leaf in leaves:
                        if not isinstance(leaf.source, exp.Table):
                            continue
                        key = f"{leaf.source.db}.{leaf.source.name}".lower()
                        if key not in upstream:
                            continue
                        layer, rel = upstream[key]
                        leaf_col = leaf.name.split(".")[-1].strip('"')
                        # The column as the upstream relation spells it (sqlglot gave it lower-cased).
                        known = bronze_cols.get(rel) if layer == "bronze" else model_cols.get(rel)
                        leaf_col = next((c for c in known or [] if c.lower() == leaf_col.lower()), leaf_col)
                        src = Node(layer, rel, leaf_col)
                        transform = (
                            "pass-through"
                            if _is_plain(expr_sql.split(".")[-1], leaf_col) and leaf_col.lower() == col.lower()
                            else expr_sql
                        )
                        lin.add(Edge(src, dst, transform, "parsed"))
                        done = True
                except Exception as err:  # noqa: BLE001 — any parse failure falls back
                    lin.fallbacks.append(f"{m.name}.{col}: {type(err).__name__}: {str(err)[:80]}")
            if not done:
                for layer, rel in upstream.values():
                    cols = bronze_cols.get(rel) if layer == "bronze" else model_cols.get(rel, [])
                    if col in (cols or []):
                        if parsed_expr is not None:
                            # Parsed, but computed from no column (e.g. a mask to NULL): the
                            # column is kept by name, its values are not.
                            lin.add(Edge(Node(layer, rel, col), dst, parsed_expr, "constant"))
                        else:
                            lin.add(Edge(Node(layer, rel, col), dst, "pass-through (by name)", "inferred"))
                        done = True
                        break
            if not done and not col.startswith("_ekos"):
                lin.unresolved.append(f"{m.name}.{col}: no upstream column")
    return lin


def _topo(models: dict[str, Model]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []

    def visit(n: str, stack: frozenset[str] = frozenset()) -> None:
        if n in seen or n in stack or n not in models:
            return
        for r in models[n].refs:
            visit(r, stack | {n})
        seen.add(n)
        out.append(n)

    for n in sorted(models):
        visit(n)
    return out


def table_edges(p: Project) -> list[tuple[str, str]]:
    """Relation-level edges for the flow diagram: source → bronze → silver → gold."""
    out: list[tuple[str, str]] = []
    for t in p.tables:
        out.append((f"source:{t.qualified}", f"bronze:{t.bronze}"))
    for m in p.models.values():
        for s, tb in m.sources:
            out.append((f"bronze:{s}.{tb}", f"{m.layer}:{m.name}"))
        for r in m.refs:
            if r in p.models:
                out.append((f"{p.models[r].layer}:{r}", f"{m.layer}:{m.name}"))
    for s in p.snapshots:
        mm = re.search(r"ref\('([^']+)'\)", s.relation)
        if mm and mm.group(1) in p.models:
            out.append((f"silver:{mm.group(1)}", f"snapshot:{s.name}"))
    return sorted(set(out))

"""``lakehouse-docs.toml``: what the generator cannot learn from the pipeline — the business
context, owners and environments. Every key is optional; missing text becomes a manual section
on the page instead of an invented sentence."""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass
class Person:
    role: str
    name: str
    contact: str = ""


@dataclass
class DocsConfig:
    project: str = "EKOS Lakehouse"
    domain: str = ""
    purpose: str = ""
    scope: str = ""
    out_of_scope: str = ""
    source_system: str = "PostgreSQL"
    source_description: str = ""
    consumers: list[dict[str, str]] = field(default_factory=list)
    owners: list[Person] = field(default_factory=list)
    glossary: dict[str, str] = field(default_factory=dict)
    environments: list[dict[str, str]] = field(default_factory=list)
    sla: dict[str, str] = field(default_factory=dict)
    retention: dict[str, str] = field(default_factory=dict)
    links: dict[str, str] = field(default_factory=dict)
    # Generation switches
    er_max_tables: int = 40          # above this, the overview ER shows keys only
    lineage_max_nodes: int = 120     # above this, the table-level flowchart is split per schema
    language: str = "en"


def load(path: Path | None) -> DocsConfig:
    cfg = DocsConfig()
    if path is None or not path.exists():
        return cfg
    raw: dict[str, Any] = tomllib.loads(path.read_text(encoding="utf-8"))
    for key, value in raw.items():
        attr = key.replace("-", "_")
        if attr == "owners":
            cfg.owners = [Person(**{k.replace("-", "_"): v for k, v in o.items()}) for o in value]
        elif hasattr(cfg, attr):
            setattr(cfg, attr, value)
    return cfg

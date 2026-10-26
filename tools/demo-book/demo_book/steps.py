"""The seven steps of the operator workflow as the book shows them (EKOS RFC 0218).

Per step: the console pages the tour photographs, the commands whose run logs the book quotes, and the
first draft of the text a person owns (what they click, what happens behind it, the caveats). The draft
is written once into a manual section; after that the book keeps whatever the maintainer wrote.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class Checkpoint:
    """One screenshot: a console page, opened at `path` (`{ws}` is the workspace), after `wait` shows.

    `tab` clicks a tab of the page first; `row` clicks a table cell with that text (selects it)."""

    id: str
    caption: str
    path: str
    wait: str
    tab: str | None = None
    row: str | None = None
    # What the window shows, by default: a page of 849 tables is longer than a screenshot can hold, and
    # a book page shows a screen, not a scroll.
    full_page: bool = False


@dataclass(frozen=True)
class Step:
    n: int
    id: str
    title: str
    checkpoints: tuple[Checkpoint, ...]
    # A run is quoted when its command path starts with one of these (`lakehouse marts`).
    commands: tuple[str, ...]
    click: str
    behind: str
    caveats: str
    facts: tuple[str, ...] = field(default=())


EXT = "/w/{ws}/x/lakehouse"

STEPS: tuple[Step, ...] = (
    Step(
        1,
        "analyse",
        "Analyse sources",
        (
            Checkpoint("01-sources", "The Sources page: the database, the code and the documents of the project",
                       "/w/{ws}/sources", "Sources"),
            Checkpoint("01-dashboard", "The workspace: the step bar and what the ledger holds", "/w/{ws}", "aria:workflow"),
        ),
        ("build", "recover", "resolve", "compile", "commit", "init-and-compile", "database test"),
        "On **Sources** the operator adds the database (its connection; the password goes to the keychain), the "
        "code repositories and the documentation, tests the connection, and clicks **Analyse**.",
        "The connectors read the sources as they are: files from the repositories, the live catalog from Oracle "
        "(inside a read-only transaction, with a read-only account). Deterministic analyzers turn them into one "
        "ledger: Java types and the application model, SQL and PL/SQL, migration scripts as schema history, "
        "documents with their tickets, and the database's own tables, routines, keys and constraints. The census "
        "the catalog reports is reconciled against what was compiled. No language model reads any of it.",
        "The analysis is as good as the parsers: what a parser skipped is counted and shown, never guessed.",
        ("workflow",),
    ),
    Step(
        2,
        "docs",
        "Documentation",
        (Checkpoint("02-commands", "The Commands page: `docs generate --layout estate`", "/w/{ws}/commands",
                    "Commands"),),
        ("docs generate",),
        "The operator runs **docs generate** with the estate layout and an output folder.",
        "EKOS writes the source estate as wiki pages from the ledger alone: the data model by area, one page per "
        "table and routine, the history of schema changes, the findings. A profile contributes counts, never "
        "values; text from a source without a known licence is left out. Sections a person writes are kept on "
        "every regeneration.",
        "Publishing to a wiki (GitHub, Azure DevOps, Confluence) is done with the lakehouse-docs publishers and is "
        "the operator's decision; the console has no Publish button yet.",
        ("workflow",),
    ),
    Step(
        3,
        "review",
        "Review and sign-off",
        (
            Checkpoint("03-scope", "Review: scope — which tables are business data", "/w/{ws}/review", "aria:sign-off",
                       tab="Scope"),
            Checkpoint("03-pii", "Review: personal data, with the proposed masking", "/w/{ws}/review", "aria:sign-off",
                       tab="PII"),
            Checkpoint("03-conflicts", "Review: where the code and the database disagree", "/w/{ws}/review",
                       "aria:sign-off", tab="Conflicts"),
        ),
        ("signoff", "review", "conflicts resolve", "identity"),
        "Per subject (scope, PII, write behaviour, glossary, conflicts, identity) the operator decides the open "
        "items, then signs the subject off by typing its name.",
        "Every decision is a ledger fact. A sign-off is bound to a digest of exactly what was shown; when a later "
        "analysis changes one of those facts, the sign-off turns stale by itself. Steps 4–7 stay blocked until "
        "every subject is signed.",
        "Hundreds of conflicts take a person's time; the review groups them, but it does not decide them.",
        ("workflow", "review"),
    ),
    Step(
        4,
        "requirements",
        "Requirements",
        (Checkpoint("04-requirements", "Requirements: each mart, its terms and how each resolved",
                    EXT + "/requirements", "Requirements (click one)", row="sales_by_month"),),
        ("lakehouse requirements",),
        "The operator describes a mart in plain words (or as Markdown), answers the questions on the page by "
        "editing a term, and approves it.",
        "The configured model (local Ollama here) drafts the mart's header from the request — the only model call "
        "in the workflow. EKOS then resolves every term against the approved source model: a column, a join path "
        "along declared keys, a filter. What does not resolve becomes a question for the operator, never for the "
        "model. Approval commits the description and records who approved which digest.",
        "A model can draft a term that resolves and is still wrong; that is why approval is a person's.",
        ("requirements",),
    ),
    Step(
        5,
        "extraction",
        "Extraction and landing",
        (Checkpoint("05-extraction", "Extraction: per table, load mode, deletes, masking, and why",
                    EXT + "/extraction", "Extraction"),),
        ("lakehouse extraction",),
        "The operator reads the proposed plan per table, changes what they disagree with (with a reason), and "
        "approves the plan.",
        "The planner proposes how each table is loaded from what step 1 found: incremental from an application "
        "watermark where the code updates it, full otherwise; how deletes are detected (bucketed key diff, with "
        "a guard against mass deletes); purged rows kept; personal data masked as step 3 decided. Approval binds "
        "the plan's digest; generating the pipeline refuses another plan.",
        "Tables whose data a type cannot carry exactly are refused and named, never rounded.",
        ("extraction",),
    ),
    Step(
        6,
        "bronze-silver",
        "Bronze and silver",
        (Checkpoint("06-lakehouse", "Lakehouse: each table's latest load and its state", EXT + "/lakehouse",
                    "Lakehouse"),),
        ("lakehouse run", "lakehouse generate", "lakehouse sync", "lakehouse run-report", "lakehouse reconcile"),
        "The operator generates the pipeline and starts the load.",
        "EKOS generates the extraction SQL, the dbt-trino silver models with their tests (keys, check lists, code "
        "lists the application declares) and the Dagster job. Dagster loads bronze into Iceberg and builds silver; "
        "the run's results come back to the ledger per table.",
        "On this laptop testbed a 165-table subset was loaded, not all 849 tables (EKOS RFC 0227).",
        ("tables",),
    ),
    Step(
        7,
        "marts",
        "Marts",
        (Checkpoint("07-marts", "Marts: a mart's SQL, terms, tests, totals and lineage", EXT + "/marts",
                    "Marts (click one)", row="sales_by_month"),),
        ("lakehouse marts", "lakehouse gold"),
        "Per approved requirement: **Propose**, **Merge** (the approval of the generated model), **Build**, "
        "**Reconcile**.",
        "Propose compiles the approved description into a dbt gold model on a branch and checks that nothing in "
        "it was invented. Merging is the approval. Build runs only that mart and its tests through Dagster. "
        "Reconcile reads each measure's total from the mart and the same total computed from silver, and shows "
        "both.",
        "Generated transformations are a template that may need tuning; terms a model drafted are marked.",
        ("marts",),
    ),
)

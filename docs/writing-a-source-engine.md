# Writing a source engine

EKOS Lakehouse reads PostgreSQL and Oracle out of the box. Any other source engine is a crate of
its own — in this repository, in a private one, sold or not — that is **registered**, never
matched: adding it changes no public crate (EKOS RFC 0255).

`examples/fixture-engine/` is a complete engine built that way, and `scripts/out-of-tree-engine.sh`
builds and tests it on every run of the gate. This guide walks through it; every item it names is
checked to exist there by that script.

## What an engine is

Four pieces, each behind a public trait:

| Piece | Trait | Crate | What it decides |
|---|---|---|---|
| `FixtureSpec` | `EngineSpec` | `ekos-lakehouse-bronze` | the type map, which columns can order a watermark, the capabilities |
| `FixtureCanon` | `CanonDialect` | `ekos-migrate-validate` | the canonical form a reconciliation renders the source side in |
| `FixtureSource` | `Source` | `ekos-lakehouse-ext` | the engine, connected read-only: catalog, reader, the read-only proof |
| `FixtureProvider` | `SourceProvider` | `ekos-lakehouse-ext` | the settings it reads, how it connects, its dialect, its conformance literals |

And one line in the binary:

```rust
ekos::app::main_with(Extensions::new(vec![Arc::new(
    LakehouseExtension::new().with_engine(Arc::new(FixtureProvider)),
)]))
```

Two providers with one engine name stop startup, naming both.

## 1. The name

`SourceEngine::from_static("fixture")`. Names follow the `[[databases]]` rule,
`[a-z0-9][a-z0-9_-]{0,62}`. A project selects the engine with `source-engine = "fixture"` in
`[extensions.lakehouse]`; a lock file records it. A build without the engine refuses both by
name: `source engine `fixture` is not in this build`.

## 2. The type map (`EngineSpec::map_column`)

Map every source type into the closed `Lossiness` and `CompareRule` sets: an engine cannot add a
member. A type with no faithful landing is `Refused` with a rationale, and refuses its table.
`FixtureSpec` refuses `money`.

Prove it with the type-map kit, `ekos_lakehouse_bronze::typemap_kit::check(&spec, &samples)`:
every sample is mapped or refused (never dropped), `NarrowingSafe` appears only with a
measurement, a refused column refuses its table, a refused column has no compare rule.

## 3. Capabilities (`EngineSpec::capabilities`)

Declare each of the ten capabilities of `ekos_kir::source_capabilities` — `provided`,
`partial { note }` or `absent { reason }`. Undeclared is absent. The build checks what it can:
`reconcile` stays provided only after a passing conformance run; `declared_keys` provided with
zero keys read is downgraded to partial. `lakehouse plan` records the result as a
`SourceCapabilities` fact, `lakehouse doctor` prints it, the estate documentation shows it at the
top of the source. Say `absent` when it is: a consumer then degrades in a stated way instead of
reading an empty list as a fact about the database.

## 4. The canonical form (`CanonDialect`)

Three methods are required — `canon_body`, `hash_hex_expr`, `prefix60_expr` — and the rest default
to portable SQL. Wrap it as `Dialect::Registered(&CANON)` with a `static CANON`: no registry.
Then prove it with the conformance kit, `ekos_migrate_validate::conformance::run(reader, literals)`,
on a live engine: every fixture case rendered over your **hand-written** typed literals must equal
the frozen canonical form, the hashes and prefixes must equal the Rust implementation's, and a
200-row bucket checksum must match. `FixtureLiterals` reuses PostgreSQL's literals because the
fixture speaks PostgreSQL's SQL; a different engine writes its own. Without a passing run the
engine's tables are never healthy.

## 5. The source (`Source`) and the provider (`SourceProvider`)

`Source::writes_refused` is required: an engine that cannot show its session is read-only is not a
source. `SourceProvider::check_settings` names the keys it refuses and the engine
(`source-password-file is not read by the fixture engine`); `doctor` adds engine checks.

## Running it

```sh
scripts/out-of-tree-engine.sh            # fmt, clippy, offline tests
EKOS_LAKEHOUSE_LIVE=1 EKOS_LH_PG_PASSWORD=… scripts/out-of-tree-engine.sh   # + the kit, live
```

What the fixture does not show yet: landing bronze and reconciling against a live Trino catalog
for the engine (the sandbox's Trino needs a catalog for the source), and a `[[databases]]` live
connector in EKOS core (RFC 0255 §3.6, not built).

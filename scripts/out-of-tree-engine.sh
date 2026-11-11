#!/usr/bin/env bash
# EKOS RFC 0255 phase 5: build and test the fixture source engine OUT OF TREE
# (examples/fixture-engine is its own Cargo workspace and depends on the public crates only),
# and check that docs/writing-a-source-engine.md names only items the fixture really defines.
#
# Live part (the conformance kit on a PostgreSQL-compatible engine): EKOS_LAKEHOUSE_LIVE=1 and
# EKOS_LH_PG_PASSWORD, as the other live suites.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$root/examples/fixture-engine"
guide="$root/docs/writing-a-source-engine.md"

# The guide is executed against the code: every `Fixture…` item it names exists in the crate.
missing=0
for item in $(grep -o 'Fixture[A-Za-z]*' "$guide" | sort -u); do
    if ! grep -qE "(struct|fn|static|const) $item\b" "$fixture/src/lib.rs"; then
        echo "guide names $item, which the fixture does not define" >&2
        missing=1
    fi
done
[ "$missing" = 0 ]

cd "$fixture"
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

#!/usr/bin/env bash
# Builds a release of EKOS Lakehouse into dist/ (RFC 0199): the full package for Linux x86_64 —
# ekos-lakehouse, a pinned Python and the EKOS web console — plus SHA256SUMS and install.sh.
#
#   scripts/release.sh <version> [<EKOS checkout>]
#
# The package is assembled by EKOS's own scripts/package-full.sh, from an EKOS checkout (default
# ../EKOS) whose console matches the EKOS crates this binary is built with: check out the commit
# Cargo.lock pins first. Nothing is uploaded; `gh release create` does that, separately.
set -euo pipefail

version="${1:?usage: scripts/release.sh <version> [<EKOS checkout>]}"
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ekos="$(cd "${2:-$repo/../EKOS}" && pwd)"
target="x86_64-unknown-linux-gnu"
out="$repo/dist"

pinned="$(sed -n 's/^source = "git+https:\/\/github.com\/alexeyban\/EKOS?branch=main#\([0-9a-f]*\)"/\1/p' \
  "$repo/Cargo.lock" | head -n 1)"
have="$(git -C "$ekos" rev-parse HEAD)"
if [ -n "$pinned" ] && [ "$pinned" != "$have" ]; then
  echo "EKOS checkout is at ${have:0:9}, Cargo.lock pins ${pinned:0:9}: the console would not" >&2
  echo "match the binary. git -C $ekos checkout $pinned (or update Cargo.lock), then rerun." >&2
  exit 2
fi

cargo build --release --manifest-path "$repo/Cargo.toml" -p ekos-lakehouse
bin="$repo/target/release/ekos-lakehouse"
"$bin" --version

mkdir -p "$out"
EKOS_PACKAGE_PROGRAM=ekos-lakehouse \
EKOS_PACKAGE_INSTALL_MD="$repo/docs/INSTALL-package.md" \
  "$ekos/scripts/package-full.sh" "$target" "$bin" "$out" "$version"

cp "$repo/install.sh" "$out/install.sh"
(cd "$out" && sha256sum "ekos-lakehouse-${version}-${target}-full.tar.gz" install.sh > SHA256SUMS)
echo
echo "dist/: $(cd "$out" && ls ekos-lakehouse-"${version}"-*.tar.gz SHA256SUMS install.sh | tr '\n' ' ')"
echo "Publish (after checking): gh release create v${version} dist/ekos-lakehouse-${version}-${target}-full.tar.gz dist/SHA256SUMS dist/install.sh"

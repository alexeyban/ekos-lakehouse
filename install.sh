#!/bin/sh
# EKOS Lakehouse installer (RFC 0199).
#
#   curl -fsSL https://raw.githubusercontent.com/alexeyban/ekos-lakehouse/main/install.sh | sh
#
# Piping a script into a shell is a real supply-chain decision. This one is short enough to read
# first:
#
#   curl -fsSL https://raw.githubusercontent.com/alexeyban/ekos-lakehouse/main/install.sh -o install.sh
#   less install.sh && sh install.sh
#
# It downloads the release's full package for this machine (ekos-lakehouse, the EKOS web console
# and the Python that runs it), verifies it against the release's SHA256SUMS **before** unpacking,
# puts it in the EKOS home ($EKOS_HOME, default ~/.ekos: bin/ and console/ only, never your
# data), then runs `ekos-lakehouse lakehouse install`, which starts the Docker stack, creates a
# first project and opens the console. It never uses sudo and writes nothing outside the home
# except what Docker itself keeps (images, containers, volumes).
#
# Arguments are passed to `lakehouse install`:
#
#   sh install.sh                                   # the sample `shop` database
#   EKOS_LH_SOURCE_PASSWORD=... sh install.sh --source db.internal:5432/sales --source-user reader
#
# Environment:
#   EKOS_LH_VERSION    tag to install (default: the latest release)
#   EKOS_HOME          the EKOS home (default: $HOME/.ekos)
#   EKOS_LH_BASE_URL   download from here instead of GitHub (a mirror, or file:///dir); the
#                      checksum check is the same
#   EKOS_LH_NO_STACK=1 install the package only; run `ekos-lakehouse lakehouse install` later

set -eu

REPO="alexeyban/ekos-lakehouse"
HOME_DIR="${EKOS_HOME:-$HOME/.ekos}"

die() {
    printf 'ekos-lakehouse install: %s\n' "$1" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "this installer needs '$1' on PATH"
}

need uname
need tar
need mkdir

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    fetch_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
    fetch_stdout() { wget -qO- "$1"; }
else
    die "this installer needs curl or wget on PATH"
fi

if [ "${EKOS_LH_NO_STACK:-}" != 1 ]; then
    command -v docker >/dev/null 2>&1 || die "EKOS Lakehouse runs its stack in Docker: install
Docker (with Compose v2) first — https://docs.docker.com/engine/install/"
fi

# ── Target ────────────────────────────────────────────────────────────────────
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64 | Linux-amd64) target="x86_64-unknown-linux-gnu" ;;
    *)
        die "no prebuilt package for $(uname -s) $(uname -m) yet (Linux x86_64 is the tested
platform). Build from source: https://github.com/$REPO#from-source"
        ;;
esac

# ── Version ───────────────────────────────────────────────────────────────────
version="${EKOS_LH_VERSION:-}"
if [ -z "$version" ]; then
    printf 'Resolving the latest release...\n'
    version="$(fetch_stdout "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
    [ -n "$version" ] || die "could not resolve the latest release. Set EKOS_LH_VERSION=vX.Y.Z"
fi
bare="${version#v}"
base="${EKOS_LH_BASE_URL:-https://github.com/$REPO/releases/download/$version}"
full="ekos-lakehouse-${bare}-${target}-full"

# ── Download, verify, unpack ──────────────────────────────────────────────────
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

printf 'Downloading %s.tar.gz (%s)...\n' "$full" "$version"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" \
    || die "could not download the checksum file for $version — refusing to install unverified"
fetch "$base/$full.tar.gz" "$tmp/$full.tar.gz" \
    || die "could not download $base/$full.tar.gz"

# The name goes into a regex; escape its dots so a line for another file cannot match.
re="$(printf '%s' "$full.tar.gz" | sed 's/[].[^$*\\/]/\\&/g')"
expected="$(sed -n "s/^\([0-9a-f]\{64\}\)[ *][ *]*${re}\$/\1/p" "$tmp/SHA256SUMS" | head -n 1)"
[ -n "$expected" ] || die "$full.tar.gz is not listed in SHA256SUMS — refusing to install unverified"
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$full.tar.gz" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp/$full.tar.gz" | cut -d' ' -f1)"
else
    die "this installer needs sha256sum or shasum to verify the download"
fi
[ "$actual" = "$expected" ] || die "checksum mismatch for $full.tar.gz
  expected $expected
  actual   $actual
Refusing to install. Report this at https://github.com/$REPO/issues"
printf 'Checksum verified.\n'

tar -C "$tmp" -xzf "$tmp/$full.tar.gz"
"$tmp/$full/bin/ekos-lakehouse" --version >/dev/null 2>&1 \
    || die "the downloaded binary does not run on this machine"

# ── Install into the home: the program, never the data ────────────────────────
mkdir -p "$HOME_DIR/bin" "$HOME_DIR/data"
# A running binary (an agent's MCP server, a pipeline run) cannot be overwritten, but it can be
# replaced by a rename and keeps running from the old file.
cp "$tmp/$full/bin/ekos-lakehouse" "$HOME_DIR/bin/ekos-lakehouse.new.$$"
chmod 755 "$HOME_DIR/bin/ekos-lakehouse.new.$$"
mv -f "$HOME_DIR/bin/ekos-lakehouse.new.$$" "$HOME_DIR/bin/ekos-lakehouse"
rm -rf "$HOME_DIR/console.new.$$"
cp -R "$tmp/$full/console" "$HOME_DIR/console.new.$$"
rm -rf "$HOME_DIR/console"
mv "$HOME_DIR/console.new.$$" "$HOME_DIR/console"
printf 'Installed ekos-lakehouse %s into %s\n' "$version" "$HOME_DIR"

if [ "${EKOS_LH_NO_STACK:-}" = 1 ]; then
    printf 'Next: %s/bin/ekos-lakehouse lakehouse install\n' "$HOME_DIR"
    exit 0
fi

# ── The stack, a first project and the console ────────────────────────────────
# Under `curl | sh` this script's stdin is the script itself; the installer reads nothing from
# it, so it gets /dev/null.
EKOS_HOME="$HOME_DIR" exec "$HOME_DIR/bin/ekos-lakehouse" lakehouse install "$@" </dev/null

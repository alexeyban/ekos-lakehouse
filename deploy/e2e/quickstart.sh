#!/bin/sh
# RFC 0196 exit: run docs/quickstart.md's marked blocks verbatim, in order, against a fresh
# project directory, then check every table of the example is healthy in bronze and silver.
# The blocks themselves create the `shop` database and its Trino catalog; this script removes
# the catalog file afterwards.
set -eu
REPO=$(cd "$(dirname "$0")/../.." && pwd)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/ekos-lh-quickstart.XXXXXX")
SCRIPT="$WORK/quickstart-blocks.sh"
trap 'rm -f "$REPO/deploy/trino/catalog/shop.properties"' EXIT

# Every ```bash block that follows a `<!-- run -->` line.
awk '
  /^<!-- run -->$/ { take = 1; next }
  take && /^```bash$/ { inblock = 1; next }
  inblock && /^```$/ { inblock = 0; take = 0; print ""; next }
  inblock { print }
' "$REPO/docs/quickstart.md" > "$SCRIPT"
blocks=$(grep -c '^<!-- run -->$' "$REPO/docs/quickstart.md")
echo "running $blocks blocks from docs/quickstart.md in $WORK"

LAKEHOUSE_REPO="$REPO" MY_PROJECT="$WORK/my-lakehouse" sh -eu "$SCRIPT" > "$WORK/out.log" 2>&1 || {
  echo "a quickstart block failed — last lines:"; tail -30 "$WORK/out.log"; exit 1; }

status=$(cd "$WORK/my-lakehouse" && "$REPO/target/release/ekos-lakehouse" lakehouse status)
echo "$status"
echo "$status" | grep -q '^4 healthy, 0 late, 0 unverified, 0 failing, 0 schema pending$' || {
  echo "expected customers and orders healthy in bronze and silver"; exit 1; }
echo "quickstart: ok (log: $WORK/out.log)"

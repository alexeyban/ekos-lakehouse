#!/bin/sh
# Loads Pagila (PostgreSQL License, github.com/devrimgunduz/pagila) into the sandbox's source
# PostgreSQL as database `pagila`: the RFC 0190 M1 exit fixture. Pinned to one commit and
# checked by SHA-256, so every run loads the same bytes.
#
#   deploy/fixtures/pagila.sh            # needs the stack up (lh-postgres)
#   deploy/fixtures/pagila.sh --reload   # drop and load again
set -eu
COMMIT=9baf49c4149e43229f6021e6218d6b2ac8ef4f34
SCHEMA_SHA=071ee940a73c8f4fad2997185788065291607e2c55559e3747959e8275391536
DATA_SHA=a88efa94c7ae8bc9cf55def4efc9f164d064d5b9cd93f11719ba3b5ace1602f7
PG=${EKOS_LH_PG_CONTAINER:-ekos-lakehouse-lh-postgres-1}
CACHE=$(dirname "$0")/.cache/pagila-$COMMIT
mkdir -p "$CACHE"

fetch() { # file sha
  if [ ! -f "$CACHE/$1" ]; then
    curl -sfL -o "$CACHE/$1.part" \
      "https://raw.githubusercontent.com/devrimgunduz/pagila/$COMMIT/$1"
    mv "$CACHE/$1.part" "$CACHE/$1"
  fi
  echo "$2  $CACHE/$1" | sha256sum -c --quiet - || { rm -f "$CACHE/$1"; exit 1; }
}
fetch pagila-schema.sql "$SCHEMA_SHA"
fetch pagila-data.sql "$DATA_SHA"

psql() { docker exec -i "$PG" psql -U ekos -v ON_ERROR_STOP=1 -q "$@"; }
exists=$(psql -d source -tAc "SELECT 1 FROM pg_database WHERE datname = 'pagila'")
if [ "$exists" = 1 ] && [ "${1:-}" != --reload ]; then
  echo "pagila is already loaded (--reload to load it again)"
  exit 0
fi
psql -d source -c "DROP DATABASE IF EXISTS pagila"
psql -d source -c "CREATE DATABASE pagila"
# The dump assigns ownership to `postgres`; the sandbox's superuser is `ekos`.
psql -d source -c "DO \$\$ BEGIN CREATE ROLE postgres; EXCEPTION WHEN duplicate_object THEN NULL; END \$\$"
psql -d pagila < "$CACHE/pagila-schema.sql"
psql -d pagila < "$CACHE/pagila-data.sql"
psql -d pagila -c "ANALYZE"
psql -d pagila -tAc "SELECT 'pagila loaded: ' || count(*) || ' tables' FROM pg_tables WHERE schemaname = 'public'"

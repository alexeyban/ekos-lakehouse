# EKOS Lakehouse @VERSION@ — install package

`@TARGET@`, built from commit `@COMMIT@`.

This package holds `bin/ekos-lakehouse` (all of EKOS plus the Lakehouse extension), the EKOS web
console in `console/`, and the Python that runs it. Nothing here needs a compiler.

## Install

You need Docker with Compose v2 and about 6 GB of free memory for the stack.

```bash
./bin/ekos-lakehouse lakehouse install
```

That starts the stack (PostgreSQL sample, Trino, Polaris, SeaweedFS, Dagster), creates the sample
project `shop`, runs its pipeline once, and opens the web console on its **Lakehouse** tab.

For your own database:

```bash
export EKOS_LH_SOURCE_PASSWORD='…'         # read once, stored in owner-only files
./bin/ekos-lakehouse lakehouse install --source db.example.com:5432/sales --source-user reader
```

Everything goes into the EKOS home (`~/.ekos`, or `$EKOS_HOME`). The full guide, with what each
step changes and how to remove it: https://github.com/alexeyban/ekos-lakehouse/blob/main/docs/install.md

## Remove

```bash
~/.ekos/bin/ekos-lakehouse lakehouse install --uninstall      # the stack (add --purge for its data)
~/.ekos/bin/ekos-lakehouse uninstall                          # the console and the program
```

# Verify a Pentaho translation against Pentaho itself

EKOS checks every translated model before proposing it: Trino must plan it, every step is
labelled, and a model that depends on something untranslated is disabled. What those checks
cannot prove is that the dbt model returns **the same rows** as your transformation. Only running
both on the same data does that — and we have not run that comparison on real Pentaho estates
yet. **If you have one, this page is how you can help.**

It takes four steps and needs a copy of the source data, Pentaho Data Integration (PDI, any
8.x/9.x/10.x), and an EKOS Lakehouse project ([install](install.md)).

## 1. Translate

```bash
ekos-lakehouse lakehouse pentaho translate <package> --out /tmp/pentaho-preview
```

Note the transformation you will compare and the gold model it became (`pentaho report
--package <package>` lists both, e.g. `fact_orders.ktr` → `gold__fact_orders`).

## 2. Run the original in PDI

Point the transformation's **output** connection at a scratch schema (never production), and its
input connections at the same source the lakehouse reads:

```bash
# PDI's command-line runner; -param sets named parameters, if the transformation has any
./pan.sh -file=/path/to/fact_orders.ktr -level=Basic
```

PDI writes the result to the scratch table (for example `scratch.fact_orders` in PostgreSQL).

## 3. Build the dbt model

Merge (or check out) the proposal branch in a copy of the pipeline repository and build just the
Pentaho models:

```bash
dbt build --select path:models/silver/pentaho path:models/gold/pentaho
```

## 4. Compare in Trino

Trino sees both: the scratch table through the source catalog, the gold model in Iceberg.
Replace the names and run:

```sql
-- the same number of rows?
SELECT
  (SELECT count(*) FROM shop.scratch.fact_orders)   AS pentaho_rows,
  (SELECT count(*) FROM iceberg.gold.fact_orders)   AS dbt_rows;

-- rows in one and not the other (compare the columns both have)
SELECT 'only in pentaho' AS side, count(*) FROM (
  SELECT order_id, customer_id, CAST(gross AS decimal(18,4)) FROM shop.scratch.fact_orders
  EXCEPT ALL
  SELECT order_id, customer_id, CAST(gross AS decimal(18,4)) FROM iceberg.gold.fact_orders)
UNION ALL
SELECT 'only in dbt', count(*) FROM (
  SELECT order_id, customer_id, CAST(gross AS decimal(18,4)) FROM iceberg.gold.fact_orders
  EXCEPT ALL
  SELECT order_id, customer_id, CAST(gross AS decimal(18,4)) FROM shop.scratch.fact_orders);
```

Two zeros mean the translation reproduced the transformation on that data. Round floating-point
columns to a fixed scale, as above: Java doubles and Trino doubles can differ in the last digit.
Technical keys of slowly changing dimensions are expected to differ (EKOS uses dbt's
`dbt_scd_id`, PDI an integer sequence): leave them out of the comparison.

## 5. Tell us

Open an issue with the **Pentaho verification** template. Send counts, step types and the
fidelity labels from `pentaho report` — **never row values**. A difference is the most useful
result there is: say which step you think caused it. The labels tell you where to look first:
a *rewritten* step names the rule it applied.

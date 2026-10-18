---
name: Pentaho verification
about: You ran a translated Pentaho transformation against PDI on the same data (docs/pentaho-verify.md)
title: "Pentaho verification: <transformation> — same | different"
labels: pentaho, verification
---

**Never paste row values or credentials.** Counts, step types and labels only.

- PDI version:
- EKOS Lakehouse version (`ekos-lakehouse --version`):
- Transformation (name is enough) and the gold model it became:
- Step types in it (from `lakehouse pentaho report --package <p>`):
- Fidelity counts (exact / rewritten / llm-drafted / gap):

**Result**

| | rows |
|---|---|
| Pentaho | |
| dbt | |
| only in Pentaho | |
| only in dbt | |

**If they differ:** which step do you suspect, and why? (A *rewritten* step names its rule.)

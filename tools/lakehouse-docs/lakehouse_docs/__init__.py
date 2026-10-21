"""EKOS Lakehouse documentation generator.

Reads what ekos-lakehouse already knows — the deployed plan (``lakehouse.lock.json``), the
generated dbt project, gold mart descriptions, change proposals, the pipeline repository's git
history and the JSON evidence of ``lakehouse status|check|reconcile|tables|proposals`` — and
writes a Markdown + Mermaid documentation set. It never connects to a database itself.
"""

__version__ = "0.1.0"

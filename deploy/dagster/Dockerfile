# Dagster for the EKOS Lakehouse sandbox. The generated pipeline project (M1) replaces the
# placeholder definitions; versions are pinned to what the generator targets.
FROM python:3.12.15-slim
# `lakehouse sync` commits and branches in the pipeline repository (RFC 0194).
RUN apt-get update && apt-get install -y --no-install-recommends git \
    && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir \
      dagster==1.13.26 dagster-webserver==1.13.26 dagster-dbt==0.29.26 dbt-trino==1.10.6 \
      trino==0.340.0 sqlfluff==4.3.0 \
      dbt-duckdb==1.11.1 duckdb==1.5.6
# EKOS RFC 0259: gold marts on DuckDB. The extensions dbt-duckdb loads (`httpfs`, `iceberg` and
# its `avro`) are installed into the image, so a run never downloads code; the generated
# profiles.yml reads this directory from EKOS_LH_DUCKDB_EXTENSIONS. DuckDB 1.5.6 is the version every reader of the marts
# file (Superset, EKOS's probe) is pinned to.
ENV EKOS_LH_DUCKDB_EXTENSIONS=/opt/duckdb/extensions
RUN python -c "import duckdb; c = duckdb.connect(); \
c.execute(\"SET extension_directory = '/opt/duckdb/extensions'\"); \
c.execute('INSTALL httpfs'); c.execute('INSTALL iceberg'); c.execute('INSTALL avro')" \
    && chmod -R a+rX /opt/duckdb
WORKDIR /opt/dagster
COPY placeholder.py .
ENV DAGSTER_HOME=/opt/dagster/home
RUN mkdir -p $DAGSTER_HOME
EXPOSE 3000
CMD ["dagster", "dev", "-h", "0.0.0.0", "-p", "3000", "-f", "placeholder.py"]

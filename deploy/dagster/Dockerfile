# Dagster for the EKOS Lakehouse sandbox. The generated pipeline project (M1) replaces the
# placeholder definitions; versions are pinned to what the generator targets.
FROM python:3.12.15-slim
RUN pip install --no-cache-dir \
      dagster==1.13.26 dagster-webserver==1.13.26 dagster-dbt==0.29.26 dbt-trino==1.10.6 \
      trino==0.340.0
WORKDIR /opt/dagster
COPY placeholder.py .
ENV DAGSTER_HOME=/opt/dagster/home
RUN mkdir -p $DAGSTER_HOME
EXPOSE 3000
CMD ["dagster", "dev", "-h", "0.0.0.0", "-p", "3000", "-f", "placeholder.py"]

//! RFC 0190 M0: how fast does Trino's `postgresql` connector load a table into Iceberg?
//!
//!   cargo run --release -p ekos-lakehouse-trino --example throughput -- <table> <splits>...
//!
//! Loads `postgresql.public.<table>` into `iceberg.bronze.tp_<splits>` once per split count: `1`
//! is a single `CREATE TABLE AS`; `n > 1` creates the table empty and runs `n` key-range
//! `INSERT … WHERE id BETWEEN …` statements concurrently (the connector reads a table through one
//! connection per statement, so ranges are the way to parallelize). Each load is checked by
//! row count. Needs the sandbox; `EKOS_LH_TRINO_URL` overrides the coordinator.

use ekos_lakehouse_trino::{Client, Connection};
use std::time::Instant;

fn client() -> Client {
    Client::new(Connection {
        base_url: std::env::var("EKOS_LH_TRINO_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:58080".into()),
        user: "ekos".into(),
        catalog: None,
        schema: None,
    })
    .unwrap()
}

fn scalar(c: &Client, sql: &str) -> i64 {
    c.execute(sql).unwrap().rows[0][0].as_i64().unwrap()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let table = args.next().expect("table name");
    let splits: Vec<u32> = args.map(|a| a.parse().expect("split count")).collect();
    let c = client();
    let src = format!("postgresql.public.{table}");
    let rows = scalar(&c, &format!("SELECT count(*) FROM {src}"));
    let (lo, hi) = {
        let r = c
            .execute(&format!("SELECT min(id), max(id) FROM {src}"))
            .unwrap();
        (
            r.rows[0][0].as_i64().unwrap(),
            r.rows[0][1].as_i64().unwrap(),
        )
    };
    c.execute("CREATE SCHEMA IF NOT EXISTS iceberg.bronze")
        .unwrap();
    println!("source {src}: {rows} rows, id {lo}..={hi}");

    for n in splits {
        let target = format!("iceberg.bronze.tp_{table}_{n}");
        c.execute(&format!("DROP TABLE IF EXISTS {target}"))
            .unwrap();
        let start = Instant::now();
        if n == 1 {
            c.execute(&format!("CREATE TABLE {target} AS SELECT * FROM {src}"))
                .unwrap();
        } else {
            c.execute(&format!(
                "CREATE TABLE {target} AS SELECT * FROM {src} WITH NO DATA"
            ))
            .unwrap();
            let width = (hi - lo + i64::from(n)) / i64::from(n);
            std::thread::scope(|s| {
                for k in 0..i64::from(n) {
                    let (a, b) = (lo + k * width, (lo + (k + 1) * width - 1).min(hi));
                    let (target, src) = (&target, &src);
                    s.spawn(move || {
                        client()
                            .execute(&format!(
                                "INSERT INTO {target} SELECT * FROM {src} \
                                 WHERE id BETWEEN {a} AND {b}"
                            ))
                            .unwrap();
                    });
                }
            });
        }
        let secs = start.elapsed().as_secs_f64();
        let loaded = scalar(&c, &format!("SELECT count(*) FROM {target}"));
        let files = scalar(
            &c,
            &format!("SELECT count(*) FROM iceberg.bronze.\"tp_{table}_{n}$files\""),
        );
        assert_eq!(loaded, rows, "{target} has {loaded} rows, source {rows}");
        println!(
            "splits {n:>2}: {secs:>7.1}s  {:>9.0} rows/s  {files} data files",
            rows as f64 / secs
        );
    }
}

//! Live cross-engine conformance tests — the end-to-end exercise of the whole
//! stack: real `PostgresEngine`/`DuckDBEngine` → introspection → `build_plan`
//! → core's `conformance_check`. These replace the old `vrtb-integ` crate.
//!
//! Prerequisites (same as before):
//!   - `docker compose up -d`            (PostgreSQL on localhost:5432)
//!   - `python testdata/seed.py`         (tables created + seeded)
//!   - the seeded DuckDB file at `data/duckdb/veritable.duckdb`
//!
//! All tests are plain `#[test]` (not `#[tokio::test]`): `PostgresEngine` owns
//! its runtime internally, so callers must NOT be inside a Tokio runtime.

use std::path::PathBuf;

use vrtb_core::build_plan;
use vrtb_core::conformance::{conformance_check, whole_table_checksum, Verdict};
use vrtb_core::engine::{ComparePlan, Engine, TableRef};
use vrtb_duck::engine::DuckDBEngine;
use vrtb_pg::engine::PostgresEngine;

const KEY: &str = "id";

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn pg() -> PostgresEngine {
    let conn = format!(
        "host={} port={} user={} password={} dbname={}",
        env("POSTGRES_HOST", "localhost"),
        env("POSTGRES_PORT", "5432"),
        env("POSTGRES_USER", "postgres"),
        env("POSTGRES_PASSWORD", "340fd5c70c687b4e622aac22df"),
        env("POSTGRES_DB", "veritable"),
    );
    PostgresEngine::connect(&conn).expect("connect to seeded Postgres")
}

fn duck() -> DuckDBEngine {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("data")
        .join("duckdb")
        .join("veritable.duckdb");
    DuckDBEngine::open_read_only(&path).expect("open seeded DuckDB file")
}

fn tref(name: &str) -> TableRef {
    TableRef { schema: None, name: name.into() }
}

/// Introspect both sides and plan the key plus every shared non-key column.
fn plan(src: &dyn Engine, src_t: &TableRef, dst: &dyn Engine, dst_t: &TableRef) -> ComparePlan {
    let src_schema = src.introspect(src_t).unwrap();
    let dst_schema = dst.introspect(dst_t).unwrap();
    let cols: Vec<String> = src_schema
        .columns
        .iter()
        .filter(|c| c.name != KEY)
        .map(|c| c.name.clone())
        .collect();
    build_plan(&src_schema, &dst_schema, Some(&cols), KEY).unwrap()
}

// ----- same-engine: DuckDB -----

#[test]
fn duck_identical_tables_match() {
    let e = duck();
    let (s, d) = (tref("customers_identical_src"), tref("customers_identical_dst"));
    let p = plan(&e, &s, &e, &d);
    let v = conformance_check(&e, &s, &e, &d, &p).unwrap();
    assert!(v.is_match(), "identical DuckDB tables must match: {v:?}");
}

#[test]
fn duck_modified_tables_differ() {
    let e = duck();
    let (s, d) = (tref("customers_src"), tref("customers_dst"));
    let p = plan(&e, &s, &e, &d);
    let v = conformance_check(&e, &s, &e, &d, &p).unwrap();
    assert!(!v.is_match(), "modified DuckDB tables must differ");
}

// ----- same-engine: Postgres -----

#[test]
fn pg_identical_tables_match() {
    let e = pg();
    let (s, d) = (tref("customers_identical_src"), tref("customers_identical_dst"));
    let p = plan(&e, &s, &e, &d);
    let v = conformance_check(&e, &s, &e, &d, &p).unwrap();
    assert!(v.is_match(), "identical PG tables must match: {v:?}");
}

#[test]
fn pg_modified_tables_differ() {
    let e = pg();
    let (s, d) = (tref("customers_src"), tref("customers_dst"));
    let p = plan(&e, &s, &e, &d);
    let v = conformance_check(&e, &s, &e, &d, &p).unwrap();
    assert!(!v.is_match(), "modified PG tables must differ");
}

// ----- cross-engine: the core conformance guarantee -----

#[test]
fn cross_engine_identical_checksums_match() {
    let p_eng = pg();
    let d_eng = duck();
    let t = tref("customers_identical_src");
    let p = plan(&p_eng, &t, &d_eng, &t);
    let v = conformance_check(&p_eng, &t, &d_eng, &t, &p).unwrap();
    match v {
        Verdict::Match => {}
        Verdict::Differ { src, dst } => {
            panic!("PG and DuckDB must agree on identical data:\n  pg={src:?}\n  duck={dst:?}")
        }
    }
}

// ----- counts reflect the seed -----

#[test]
fn row_counts_match_seed() {
    let p_eng = pg();
    let d_eng = duck();
    let t = tref("customers_identical_src");
    let p = plan(&p_eng, &t, &d_eng, &t);

    let pg_ident = whole_table_checksum(&p_eng, &t, &p).unwrap();
    assert_eq!(pg_ident.count, 10_000, "seed creates 10k identical rows");

    let duck_ident = whole_table_checksum(&d_eng, &t, &p).unwrap();
    assert_eq!(duck_ident.count, 10_000);

    // Modified dst: 10_000 - 100 deletes + 150 inserts = 10_050.
    let dst = tref("customers_dst");
    let p_dst = plan(&p_eng, &dst, &p_eng, &dst);
    let pg_dst = whole_table_checksum(&p_eng, &dst, &p_dst).unwrap();
    assert_eq!(pg_dst.count, 10_050, "dst = 10k - 100 deletes + 150 inserts");
}

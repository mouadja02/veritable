//! Live cross-engine conformance tests — the end-to-end exercise of the whole
//! stack: real `PostgresEngine`/`DuckDBEngine` → introspection → `plan`
//! → core's `conformance_check`. These replace the old `vrtb-integ` crate.
//!
//! Prerequisites (same as before):
//!   - `docker compose up -d`            (PostgreSQL on localhost:5432)
//!   - `python testdata/seed.py`         (tables created + seeded)
//!   - the seeded DuckDB file at `data/duckdb/veritable.duckdb`
//!
//! All tests are plain `#[test]` (not `#[tokio::test]`): `PostgresEngine` owns
//! its runtime internally, so callers must NOT be inside a Tokio runtime.

use load_dotenv::load_dotenv;
use std::path::PathBuf;

use vrtb_core::conformance::{Verdict, conformance_check, whole_table_checksum};
use vrtb_core::engine::{Engine, TableRef};
use vrtb_core::format::Format;
use vrtb_core::plan;
use vrtb_duck::engine::DuckDBEngine;
use vrtb_pg::engine::PostgresEngine;

const KEY: &str = "id";

fn env(name: &str, default: &str) -> String {
    load_dotenv!();
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

fn duck_rw() -> DuckDBEngine {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("data")
        .join("duckdb")
        .join("veritable.duckdb");
    DuckDBEngine::new(&path).expect("open seeded DuckDB file read-write")
}

fn tref(name: &str) -> TableRef {
    TableRef {
        schema: None,
        name: name.into(),
    }
}

// ----- same-engine: DuckDB -----

#[test]
fn duck_identical_tables_match() {
    let e = duck();
    let (s, d) = (
        tref("customers_identical_src"),
        tref("customers_identical_dst"),
    );
    let p = plan(&e, &s, &e, &d, KEY).expect("plan");
    let v = conformance_check(&e, &s, &e, &d, &p, Format::Summary, None).unwrap();
    assert!(v.is_match(), "identical DuckDB tables must match: {v:?}");
}

#[test]
fn duck_modified_tables_differ() {
    let e = duck();
    let (s, d) = (tref("customers_src"), tref("customers_dst"));
    let p = plan(&e, &s, &e, &d, KEY).expect("plan");
    let v = conformance_check(&e, &s, &e, &d, &p, Format::Summary, None).unwrap();
    assert!(!v.is_match(), "modified DuckDB tables must differ");
}

// ----- same-engine: Postgres -----

#[test]
fn pg_identical_tables_match() {
    let e = pg();
    let (s, d) = (
        tref("customers_identical_src"),
        tref("customers_identical_dst"),
    );
    let p = plan(&e, &s, &e, &d, KEY).expect("plan");
    let v = conformance_check(&e, &s, &e, &d, &p, Format::Summary, None).unwrap();
    assert!(v.is_match(), "identical PG tables must match: {v:?}");
}

#[test]
fn pg_modified_tables_differ() {
    let e = pg();
    let (s, d) = (tref("customers_src"), tref("customers_dst"));
    let p = plan(&e, &s, &e, &d, KEY).expect("plan");
    let v = conformance_check(&e, &s, &e, &d, &p, Format::Summary, None).unwrap();
    assert!(!v.is_match(), "modified PG tables must differ");
}

// ----- cross-engine: the core conformance guarantee -----

#[test]
fn cross_engine_identical_checksums_match() {
    let p_eng = pg();
    let d_eng = duck();
    let t = tref("customers_identical_src");
    let p = plan(&p_eng, &t, &d_eng, &t, KEY).expect("plan");
    let v = conformance_check(&p_eng, &t, &d_eng, &t, &p, Format::Summary, None).unwrap();
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
    let p = plan(&p_eng, &t, &d_eng, &t, KEY).expect("plan");

    let pg_ident = whole_table_checksum(&p_eng, &t, &p).unwrap();
    assert_eq!(pg_ident.count, 10_000, "seed creates 10k identical rows");

    let duck_ident = whole_table_checksum(&d_eng, &t, &p).unwrap();
    assert_eq!(duck_ident.count, 10_000);

    // Modified dst: 10_000 - 100 deletes + 150 inserts = 10_050.
    let dst = tref("customers_dst");
    let p_dst = plan(&p_eng, &dst, &p_eng, &dst, KEY).expect("plan");
    let pg_dst = whole_table_checksum(&p_eng, &dst, &p_dst).unwrap();
    assert_eq!(
        pg_dst.count, 10_050,
        "dst = 10k - 100 deletes + 150 inserts"
    );
}

// ----- materialize -----

#[test]
fn pg_materialize_writes_diff_table() {
    let e = pg();
    e.execute("DROP TABLE IF EXISTS vrtb_mat_pg").unwrap();
    let (s, d) = (tref("customers_src"), tref("customers_dst"));
    let p = plan(&e, &s, &e, &d, KEY).expect("plan");
    let v = conformance_check(&e, &s, &e, &d, &p, Format::Summary, Some(&tref("vrtb_mat_pg")))
        .unwrap();
    assert!(!v.is_match());

    let counts = e
        .execute("SELECT \"op\", COUNT(*) FROM vrtb_mat_pg GROUP BY \"op\" ORDER BY \"op\"")
        .unwrap();
    assert_eq!(
        counts,
        vec![
            vec!["+".to_string(), "150".to_string()],
            vec!["-".to_string(), "100".to_string()],
            vec!["~".to_string(), "197".to_string()],
        ]
    );
    let jsons = e
        .execute(
            "SELECT CAST(src_row AS VARCHAR), CAST(dst_row AS VARCHAR) \
             FROM vrtb_mat_pg WHERE \"op\" = '~' LIMIT 1",
        )
        .unwrap();
    assert!(!jsons[0][0].is_empty() && !jsons[0][1].is_empty());
    assert_ne!(jsons[0][0], jsons[0][1], "a '~' row must differ between sides");
    e.execute("DROP TABLE vrtb_mat_pg").unwrap();
}

#[test]
fn duck_materialize_writes_diff_table() {
    let e = duck_rw();
    e.execute("DROP TABLE IF EXISTS vrtb_mat_duck").unwrap();
    let (s, d) = (tref("customers_src"), tref("customers_dst"));
    let p = plan(&e, &s, &e, &d, KEY).expect("plan");
    let v = conformance_check(&e, &s, &e, &d, &p, Format::Summary, Some(&tref("vrtb_mat_duck")))
        .unwrap();
    assert!(!v.is_match());

    let counts = e
        .execute("SELECT \"op\", COUNT(*) FROM vrtb_mat_duck GROUP BY \"op\" ORDER BY \"op\"")
        .unwrap();
    assert_eq!(
        counts,
        vec![
            vec!["+".to_string(), "150".to_string()],
            vec!["-".to_string(), "100".to_string()],
            vec!["~".to_string(), "197".to_string()],
        ]
    );
    let jsons = e
        .execute(
            "SELECT CAST(src_row AS VARCHAR), CAST(dst_row AS VARCHAR) \
             FROM vrtb_mat_duck WHERE \"op\" = '~' LIMIT 1",
        )
        .unwrap();
    assert!(!jsons[0][0].is_empty() && !jsons[0][1].is_empty());
    assert_ne!(jsons[0][0], jsons[0][1], "a '~' row must differ between sides");
    e.execute("DROP TABLE vrtb_mat_duck").unwrap();
}

#[test]
fn pg_materialize_fails_if_table_exists() {
    let e = pg();
    e.execute("DROP TABLE IF EXISTS vrtb_mat_exists").unwrap();
    e.execute("CREATE TABLE vrtb_mat_exists (x INT)").unwrap();
    let (s, d) = (tref("customers_src"), tref("customers_dst"));
    let p = plan(&e, &s, &e, &d, KEY).expect("plan");
    let r = conformance_check(&e, &s, &e, &d, &p, Format::Summary, Some(&tref("vrtb_mat_exists")));
    assert!(r.is_err(), "existing target must fail, not be replaced");
    // Untouched: still zero rows, still the original column.
    let n = e.execute("SELECT COUNT(*) FROM vrtb_mat_exists").unwrap();
    assert_eq!(n[0][0], "0");
    e.execute("DROP TABLE vrtb_mat_exists").unwrap();
}

#[test]
fn cross_engine_materialize_is_rejected() {
    let p_eng = pg();
    let d_eng = duck();
    let (s, d) = (tref("customers_src"), tref("customers_dst"));
    let p = plan(&p_eng, &s, &d_eng, &d, KEY).expect("plan");
    let r = conformance_check(&p_eng, &s, &d_eng, &d, &p, Format::Summary, Some(&tref("nope")));
    assert!(r.is_err(), "cross-engine materialize must be a hard error");
}

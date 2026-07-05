//! Parse `--src` / `--dst` target strings into an engine + table, and build the
//! concrete engine. This is the only place the CLI binds to specific adapters.
//!
//! Target syntax (`<connection>#<table>`):
//!   postgres://user:pass@host:port/db#schema.table
//!   postgresql://user:pass@host:port/db#table
//!   duckdb:/path/to/file.duckdb#schema.table
//!
//! The `#` separates the connection from the table; an optional `schema.` prefix
//! on the table sets `TableRef::schema`.

use std::path::PathBuf;

use vrtb_core::engine::{Engine, TableRef};
use vrtb_core::error::{Result, VeritableError};
use vrtb_duck::engine::DuckDBEngine;
use vrtb_pg::engine::PostgresEngine;

#[derive(Debug)]
pub enum EngineSpec {
    Postgres(String),
    DuckDb(PathBuf),
}

pub struct Target {
    pub spec: EngineSpec,
    pub table: TableRef,
}

pub fn parse_target(raw: &str) -> Result<Target> {
    let (conn, table_str) = raw
        .rsplit_once('#')
        .ok_or_else(|| VeritableError::Config(format!("target {raw:?} is missing '#<table>'")))?;
    if table_str.is_empty() {
        return Err(VeritableError::Config(format!(
            "target {raw:?} has an empty table"
        )));
    }

    let spec = if conn.starts_with("postgres://") || conn.starts_with("postgresql://") {
        EngineSpec::Postgres(conn.to_string())
    } else if let Some(path) = conn.strip_prefix("duckdb:") {
        if path.is_empty() {
            return Err(VeritableError::Config(format!(
                "target {raw:?} has an empty duckdb path"
            )));
        }
        EngineSpec::DuckDb(PathBuf::from(path))
    } else {
        return Err(VeritableError::Config(format!(
            "target {raw:?}: unrecognized engine; expected 'postgres://…' or 'duckdb:…'"
        )));
    };

    Ok(Target {
        spec,
        table: parse_table(table_str),
    })
}

pub fn parse_table(s: &str) -> TableRef {
    match s.split_once('.') {
        Some((schema, name)) => TableRef {
            schema: Some(schema.into()),
            name: name.into(),
        },
        None => TableRef {
            schema: None,
            name: s.into(),
        },
    }
}

/// True when both specs are the SAME DuckDB file. A writable DuckDB connection
/// holds an exclusive file lock, so materialize must reuse one connection for
/// both sides instead of opening the file twice.
pub fn same_duckdb_file(a: &EngineSpec, b: &EngineSpec) -> bool {
    matches!((a, b), (EngineSpec::DuckDb(p1), EngineSpec::DuckDb(p2)) if p1 == p2)
}

pub fn build_engine(spec: &EngineSpec, writable: bool) -> Result<Box<dyn Engine>> {
    match spec {
        EngineSpec::Postgres(dsn) => {
            let engine = PostgresEngine::connect(dsn)?;
            Ok(Box::new(engine))
        }
        EngineSpec::DuckDb(path) => {
            let engine = if writable {
                DuckDBEngine::new(path)
            } else {
                DuckDBEngine::open_read_only(path)
            }
            .map_err(|e| VeritableError::Config(e.to_string()))?;
            Ok(Box::new(engine))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_postgres_target_with_schema() {
        let t = parse_target("postgres://u:p@localhost:5432/db#public.customers").unwrap();
        assert!(matches!(t.spec, EngineSpec::Postgres(_)));
        assert_eq!(t.table.schema.as_deref(), Some("public"));
        assert_eq!(t.table.name, "customers");
    }

    #[test]
    fn parses_duckdb_target_without_schema() {
        let t = parse_target("duckdb:data/veritable.duckdb#customers_src").unwrap();
        match t.spec {
            EngineSpec::DuckDb(p) => assert_eq!(p, PathBuf::from("data/veritable.duckdb")),
            _ => panic!("expected duckdb"),
        }
        assert_eq!(t.table.schema, None);
        assert_eq!(t.table.name, "customers_src");
    }

    #[test]
    fn rejects_missing_table() {
        assert!(parse_target("duckdb:data/veritable.duckdb").is_err());
    }

    #[test]
    fn rejects_unknown_engine() {
        assert!(parse_target("mysql://x#t").is_err());
    }

    #[test]
    fn parse_table_splits_schema() {
        let t = parse_table("main.report");
        assert_eq!(t.schema.as_deref(), Some("main"));
        assert_eq!(t.name, "report");
        let t = parse_table("report");
        assert_eq!(t.schema, None);
        assert_eq!(t.name, "report");
    }

    #[test]
    fn same_duckdb_file_matches_paths_only() {
        let d1 = EngineSpec::DuckDb(PathBuf::from("a.duckdb"));
        let d2 = EngineSpec::DuckDb(PathBuf::from("a.duckdb"));
        let d3 = EngineSpec::DuckDb(PathBuf::from("b.duckdb"));
        let p = EngineSpec::Postgres("postgres://x".into());
        assert!(same_duckdb_file(&d1, &d2));
        assert!(!same_duckdb_file(&d1, &d3));
        assert!(!same_duckdb_file(&d1, &p));
        assert!(!same_duckdb_file(&p, &p));
    }
}

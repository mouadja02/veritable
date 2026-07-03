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

fn parse_table(s: &str) -> TableRef {
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

pub fn build_engine(spec: &EngineSpec) -> Result<Box<dyn Engine>> {
    match spec {
        EngineSpec::Postgres(dsn) => {
            let engine = PostgresEngine::connect(dsn)?;
            Ok(Box::new(engine))
        }
        EngineSpec::DuckDb(path) => {
            let engine = DuckDBEngine::open_read_only(path)
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
}

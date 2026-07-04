// DuckDB Engine adapter — implements vrtb_core::engine::Engine.
use std::path;

use duckdb::Connection;
use duckdb::types::Value;

use vrtb_core::engine::{ColumnSchema, Dialect, Engine, LogicalType, TableRef, TableSchema};
use vrtb_core::error::{Result as CoreResult, VeritableError};
use vrtb_utils::sql::from_table;

use crate::dialect::DuckDBDialect;

pub struct DuckDBEngine {
    conn: Connection,
    dialect: DuckDBDialect,
}

impl DuckDBEngine {
    // Open (or create) a DuckDB database file read-write.
    pub fn new(path: &path::Path) -> duckdb::Result<Self> {
        let conn = Connection::open(path)?;
        Ok(DuckDBEngine {
            conn,
            dialect: DuckDBDialect,
        })
    }

    // Open an existing DuckDB database file read-only — the safe mode for
    // comparisons, which never mutate the data under inspection.
    pub fn open_read_only(path: &path::Path) -> duckdb::Result<Self> {
        let config = duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?;
        let conn = Connection::open_with_flags(path, config)?;
        Ok(DuckDBEngine {
            conn,
            dialect: DuckDBDialect,
        })
    }
}

fn duck_err(e: duckdb::Error) -> VeritableError {
    VeritableError::Query(e.to_string())
}

// Stringify a DuckDB cell positionally. Scalars render to their natural text;
// the checksum path only ever sees BigInt (cnt) and Text (the two sum halves).
// NULL becomes the empty string — fine here because the checksum columns are
// COALESCE'd and never null. Temporal/nested values fall back to Debug (a known
// limitation for generic `execute`, not exercised by the conformance path).
fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Boolean(b) => b.to_string(),
        Value::TinyInt(n) => n.to_string(),
        Value::SmallInt(n) => n.to_string(),
        Value::Int(n) => n.to_string(),
        Value::BigInt(n) => n.to_string(),
        Value::HugeInt(n) => n.to_string(),
        Value::UTinyInt(n) => n.to_string(),
        Value::USmallInt(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::UBigInt(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(f) => f.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::Text(s) => s.clone(),
        Value::Enum(s) => s.clone(),
        Value::Blob(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
        other => format!("{other:?}"),
    }
}

// Parse the scale out of a `DECIMAL(p, s)` / `NUMERIC(p, s)` spelling.
fn parse_scale(upper: &str) -> Option<u8> {
    let inside = upper.split('(').nth(1)?.trim_end_matches(')');
    inside.split(',').nth(1)?.trim().parse().ok()
}

// Map a DuckDB declared type string to a Veritable [`LogicalType`].
fn map_type(raw: &str) -> CoreResult<LogicalType> {
    let upper = raw.trim().to_uppercase();
    let base = upper.split('(').next().unwrap_or("").trim();
    let ty = match base {
        "TINYINT" | "SMALLINT" | "INTEGER" | "INT" | "BIGINT" | "HUGEINT" | "UTINYINT"
        | "USMALLINT" | "UINTEGER" | "UBIGINT" | "UHUGEINT" => LogicalType::Int,
        "VARCHAR" | "TEXT" | "STRING" | "CHAR" | "BPCHAR" => LogicalType::String,
        "BOOLEAN" | "BOOL" | "LOGICAL" => LogicalType::Boolean,
        "TIMESTAMP"
        | "DATETIME"
        | "TIMESTAMP WITH TIME ZONE"
        | "TIMESTAMPTZ"
        | "TIMESTAMP_NS"
        | "TIMESTAMP_MS"
        | "TIMESTAMP_S" => LogicalType::Timestamp { precision: 6 },
        "DECIMAL" | "NUMERIC" => LogicalType::Decimal {
            scale: parse_scale(&upper).unwrap_or(0),
        },
        "BLOB" | "BYTEA" | "BINARY" | "VARBINARY" => LogicalType::Binary,
        _ => {
            return Err(VeritableError::Schema(format!(
                "unsupported DuckDB type: {raw}"
            )));
        }
    };
    Ok(ty)
}

impl Engine for DuckDBEngine {
    fn name(&self) -> &str {
        "DuckDB"
    }

    fn introspect(&self, table: &TableRef) -> CoreResult<TableSchema> {
        let qualified = from_table(table);
        // `notnull` must be quoted: DuckDB parses bare `notnull` as the
        // Postgres-style postfix operator (`x NOTNULL`), not an identifier.
        let mut stmt = self
            .conn
            .prepare("SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?)")
            .map_err(duck_err)?;
        let mapped = stmt
            .query_map([&qualified], |row| {
                Ok((
                    row.get::<_, String>(0)?,         // name
                    row.get::<_, String>(1)?,         // type
                    row.get::<_, bool>(2)?,           // notnull
                    row.get::<_, Option<String>>(3)?, // dflt_value
                    row.get::<_, bool>(4)?,           // pk
                ))
            })
            .map_err(duck_err)?;

        let mut columns = Vec::new();
        for row in mapped {
            let (name, ty, notnull, default_value, primary_key) = row.map_err(duck_err)?;
            columns.push(ColumnSchema {
                name,
                ty: map_type(&ty)?,
                nullable: !notnull,
                default_value,
                primary_key,
            });
        }
        Ok(TableSchema { columns })
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    fn execute(&self, sql: &str) -> CoreResult<Vec<Vec<String>>> {
        let mut stmt = self.conn.prepare(sql).map_err(duck_err)?;
        let mut rows = stmt.query([]).map_err(duck_err)?;
        // DuckDB only knows the column count after the query has executed, so
        // read it from the live result rather than the prepared statement.
        let ncols = rows.as_ref().map(|s| s.column_count()).unwrap_or(0);
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(duck_err)? {
            let mut r = Vec::with_capacity(ncols);
            for i in 0..ncols {
                let v: Value = row.get(i).map_err(duck_err)?;
                r.push(value_to_string(&v));
            }
            out.push(r);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrtb_core::engine::{Engine, LogicalType, TableRef};

    fn tref(name: &str) -> TableRef {
        TableRef {
            schema: None,
            name: name.into(),
        }
    }

    #[test]
    fn execute_returns_stringified_rows() {
        let dir = std::env::temp_dir().join("vrtb_test");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test_open.db");
        let engine = DuckDBEngine::new(&db_path).unwrap();
        engine
            .conn
            .execute("CREATE TABLE IF NOT EXISTS test (id INTEGER)", [])
            .unwrap();
        engine.conn.execute("DELETE FROM test", []).unwrap();
        engine
            .conn
            .execute("INSERT INTO test (id) VALUES (1)", [])
            .unwrap();

        let rows = engine.execute("SELECT id FROM test").unwrap();
        assert_eq!(rows, vec![vec!["1".to_string()]]);

        drop(engine);
        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn introspect_maps_logical_types() {
        let dir = std::env::temp_dir().join("vrtb_test");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test_introspect.db");
        let engine = DuckDBEngine::new(&db_path).unwrap();
        engine
            .conn
            .execute(
                "CREATE TABLE IF NOT EXISTS test (id INTEGER, name VARCHAR PRIMARY KEY)",
                [],
            )
            .unwrap();

        let schema = engine.introspect(&tref("test")).unwrap();
        assert_eq!(schema.columns.len(), 2);
        assert_eq!(schema.columns[0].name, "id");
        assert_eq!(schema.columns[0].ty, LogicalType::Int);
        assert!(!schema.columns[0].primary_key);
        assert_eq!(schema.columns[1].name, "name");
        assert_eq!(schema.columns[1].ty, LogicalType::String);
        assert!(schema.columns[1].primary_key);

        drop(engine);
        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn map_type_handles_decimal_scale() {
        assert_eq!(
            map_type("DECIMAL(12,2)").unwrap(),
            LogicalType::Decimal { scale: 2 }
        );
        assert_eq!(
            map_type("TIMESTAMP").unwrap(),
            LogicalType::Timestamp { precision: 6 }
        );
        assert_eq!(map_type("BOOLEAN").unwrap(), LogicalType::Boolean);
    }
}

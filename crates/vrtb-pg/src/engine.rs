// PostgreSQL Engine adapter — implements vrtb_core::engine::Engine.
//
// tokio-postgres is async, but the core `Engine` trait is synchronous (so the
// engine-agnostic conformance logic in vrtb-core stays driver-free). We bridge
// the two by giving each engine its OWN current-thread Tokio runtime: the
// connection task is spawned onto it, and every sync call `block_on`s a query,
// which also drives the spawned connection task. Callers must therefore NOT be
// inside another Tokio runtime (CLI integration tests use plain `#[test]`).

use tokio::runtime::Runtime;
use tokio_postgres::{Client, NoTls, Row};

use vrtb_core::engine::{ColumnSchema, Dialect, Engine, LogicalType, TableRef, TableSchema};
use vrtb_core::error::{Result as CoreResult, VeritableError};
use vrtb_utils::sql::from_table;

use crate::dialect::PgDialect;

pub struct PostgresEngine {
    runtime: Runtime,
    client: Client,
    dialect: PgDialect,
}

impl PostgresEngine {
    // Connect using any tokio-postgres config string — libpq `key=value` form
    // or a `postgres://user:pass@host:port/db` URL. The conn string is NOT
    // echoed into errors — it can carry a password.
    pub fn connect(conn_str: &str) -> CoreResult<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VeritableError::Connectivity(format!("tokio runtime: {e}")))?;

        let client = runtime.block_on(async {
            let (client, connection) = tokio_postgres::connect(conn_str, NoTls)
                .await
                .map_err(|e| VeritableError::Connectivity(format!("postgres connect: {e}")))?;
            // Drive the connection on this engine's runtime; it makes progress
            // whenever a later block_on(query) polls the runtime.
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    eprintln!("postgres connection error: {e}");
                }
            });
            Ok::<_, VeritableError>(client)
        })?;

        Ok(PostgresEngine {
            runtime,
            client,
            dialect: PgDialect,
        })
    }

    // Convenience constructor mirroring common discrete parameters.
    pub fn new(
        host: &str,
        port: u16,
        user: &str,
        password: &str,
        dbname: &str,
    ) -> CoreResult<Self> {
        Self::connect(&format!(
            "host={host} port={port} user={user} password={password} dbname={dbname}"
        ))
    }
}

fn query_err(e: tokio_postgres::Error) -> VeritableError {
    VeritableError::Query(e.to_string())
}

// Stringify a Postgres cell by column type. Covers the types the checksum query
// returns (int8 cnt, text sums) plus common scalars; NULL → "". Unsupported
// types (e.g. numeric, timestamp) error out — a documented limit of generic
// `execute`, not hit by the conformance path.
fn pg_value_to_string(row: &Row, i: usize) -> CoreResult<String> {
    let ty = row.columns()[i].type_();
    let s = match ty.name() {
        "bool" => row
            .get::<_, Option<bool>>(i)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        "int2" => row
            .get::<_, Option<i16>>(i)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        "int4" => row
            .get::<_, Option<i32>>(i)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        "int8" => row
            .get::<_, Option<i64>>(i)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        "float4" => row
            .get::<_, Option<f32>>(i)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        "float8" => row
            .get::<_, Option<f64>>(i)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        "text" | "varchar" | "bpchar" | "name" => {
            row.get::<_, Option<String>>(i).unwrap_or_default()
        }
        other => {
            return Err(VeritableError::Query(format!(
                "unsupported Postgres column type {other:?} for generic stringify"
            )));
        }
    };
    Ok(s)
}

// Parse the scale out of a `numeric(p, s)` spelling.
fn parse_scale(s: &str) -> Option<u8> {
    let inside = s.split('(').nth(1)?.trim_end_matches(')');
    inside.split(',').nth(1)?.trim().parse().ok()
}

// Map a Postgres `format_type` string to a Veritable [`LogicalType`].
fn map_pg_type(raw: &str) -> CoreResult<LogicalType> {
    let s = raw.trim().to_lowercase();
    let ty = if s.starts_with("integer")
        || s.starts_with("bigint")
        || s.starts_with("smallint")
        || s.starts_with("int")
    {
        LogicalType::Int
    } else if s.starts_with("numeric") || s.starts_with("decimal") {
        LogicalType::Decimal {
            scale: parse_scale(&s).unwrap_or(0),
        }
    } else if s.starts_with("timestamp") {
        LogicalType::Timestamp { precision: 6 }
    } else if s.starts_with("character varying")
        || s.starts_with("varchar")
        || s == "text"
        || s.starts_with("character")
        || s.starts_with("char")
        || s.starts_with("bpchar")
        || s == "name"
    {
        LogicalType::String
    } else if s.starts_with("boolean") || s == "bool" {
        LogicalType::Boolean
    } else if s.starts_with("bytea") {
        LogicalType::Binary
    } else {
        return Err(VeritableError::Schema(format!(
            "unsupported Postgres type: {raw}"
        )));
    };
    Ok(ty)
}

const INTROSPECT_SQL: &str = "SELECT
    a.attname AS name,
    format_type(a.atttypid, a.atttypmod) AS data_type,
    a.attnotnull AS not_null,
    pg_get_expr(d.adbin, d.adrelid) AS default_value,
    COALESCE(i.indisprimary, false) AS primary_key
FROM pg_attribute a
LEFT JOIN pg_attrdef d ON a.attrelid = d.adrelid AND a.attnum = d.adnum
LEFT JOIN pg_index i ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) AND i.indisprimary
WHERE a.attrelid = $1::text::regclass AND a.attnum > 0 AND NOT a.attisdropped
ORDER BY a.attnum";

impl Engine for PostgresEngine {
    fn name(&self) -> &str {
        "Postgres"
    }

    fn introspect(&self, table: &TableRef) -> CoreResult<TableSchema> {
        let qualified = from_table(table);
        let rows = self
            .runtime
            .block_on(self.client.query(INTROSPECT_SQL, &[&qualified]))
            .map_err(query_err)?;

        let mut columns = Vec::with_capacity(rows.len());
        for row in &rows {
            let name: String = row.get("name");
            let data_type: String = row.get("data_type");
            let not_null: bool = row.get("not_null");
            let default_value: Option<String> = row.get("default_value");
            let primary_key: bool = row.get("primary_key");
            columns.push(ColumnSchema {
                name,
                ty: map_pg_type(&data_type)?,
                nullable: !not_null,
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
        let rows = self
            .runtime
            .block_on(self.client.query(sql, &[]))
            .map_err(query_err)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut r = Vec::with_capacity(row.len());
            for i in 0..row.len() {
                r.push(pg_value_to_string(row, i)?);
            }
            out.push(r);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vrtb_core::engine::LogicalType;

    #[test]
    fn maps_common_pg_types() {
        assert_eq!(map_pg_type("integer").unwrap(), LogicalType::Int);
        assert_eq!(map_pg_type("bigint").unwrap(), LogicalType::Int);
        assert_eq!(
            map_pg_type("numeric(12,2)").unwrap(),
            LogicalType::Decimal { scale: 2 }
        );
        assert_eq!(
            map_pg_type("timestamp without time zone").unwrap(),
            LogicalType::Timestamp { precision: 6 }
        );
        assert_eq!(
            map_pg_type("character varying(255)").unwrap(),
            LogicalType::String
        );
        assert_eq!(map_pg_type("text").unwrap(), LogicalType::String);
        assert_eq!(map_pg_type("boolean").unwrap(), LogicalType::Boolean);
        assert_eq!(map_pg_type("bytea").unwrap(), LogicalType::Binary);
        assert!(map_pg_type("json").is_err());
    }
}

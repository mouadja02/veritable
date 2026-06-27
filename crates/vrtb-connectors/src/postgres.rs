// PostgreSQL Engine adapter — implements vrtb_core::engine::Engine
use tokio_postgres::{Client, Error};
use dotenvy;

pub struct PostgresEngine {
    conn: Client,
}

impl PostgresEngine {
    pub async fn new(host: &str, user: &str, password: &str, dbname: &str) -> Result<Self, Error> {
        let conn_str = format!("host={} user={} password={} dbname={}", host, user, password, dbname);
        let (client, connection) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls).await?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });
        Ok(PostgresEngine { conn: client })
    }
}

struct PostgresColumn {
    name: String,
    data_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key: bool,
}
trait PostgresEngineExt {
    async fn introspect(&self, table: &str) -> Result<Vec<PostgresColumn>, Error>;
    async fn execute(&self, sql: &str) -> Result<(), Error>;
}

impl PostgresEngineExt for PostgresEngine {
    async fn introspect(&self, table: &str) -> Result<Vec<PostgresColumn>, Error> {
        let query = format!("SELECT
    a.attname AS name,
    format_type(a.atttypid, a.atttypmod) AS data_type,
    a.attnotnull AS not_null,
    pg_get_expr(d.adbin, d.adrelid) AS default_value,
    COALESCE(i.indisprimary, false) AS primary_key
FROM
    pg_attribute a
LEFT JOIN
    pg_attrdef d ON a.attrelid = d.adrelid AND a.attnum = d.adnum
LEFT JOIN
    pg_index i ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) AND i.indisprimary
WHERE
    a.attrelid = '{}'::regclass
    AND a.attnum > 0
    AND NOT a.attisdropped
ORDER BY
    a.attnum", table);
        let rows = self.conn.query(&query, &[]).await?;
        let columns: Vec<PostgresColumn> = rows.iter().map(|row| {
            PostgresColumn {
                name: row.get("name"),
                data_type: row.get("data_type"),
                not_null: row.get("not_null"),
                default_value: row.get("default_value"),
                primary_key: row.get("primary_key"),
            }
        }).collect();
        Ok(columns)
    }

    async fn execute(&self, sql: &str) -> Result<(), Error> {
        self.conn.execute(sql, &[]).await?;
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connection() {
        // Connect to docker hosted Postgres database
        // Load from .env the following variables: POSTGRES_USER, POSTGRES_PASSWORD, POSTGRES_DB
        dotenvy::from_path("../../.env").unwrap();
        let user = std::env::var("POSTGRES_USER").unwrap();
        let password = std::env::var("POSTGRES_PASSWORD").unwrap();
        let dbname = std::env::var("POSTGRES_DB").unwrap();
        let engine = PostgresEngine::new("localhost", &user, &password, &dbname).await.unwrap();
        // Check that we can interact with a table
        engine.execute("DROP TABLE IF EXISTS test_open").await.unwrap();
        engine.execute("CREATE TABLE test_open (id INTEGER)").await.unwrap();
        engine.execute("INSERT INTO test_open (id) VALUES (1)").await.unwrap();
        // Check that we can execute a simple query
        let row = engine.conn.query_one("SELECT * FROM test_open", &[]).await.unwrap();
        let val: i32 = row.get(0);
        assert_eq!(val, 1);
        // Cleanup
        engine.execute("DROP TABLE IF EXISTS test_open").await.unwrap();
        drop(engine);
    }

    #[tokio::test]
    async fn test_introspect() {  
        dotenvy::from_path("../../.env").unwrap();
        let user = std::env::var("POSTGRES_USER").unwrap();
        let password = std::env::var("POSTGRES_PASSWORD").unwrap();
        let dbname = std::env::var("POSTGRES_DB").unwrap();
        let engine = PostgresEngine::new("localhost", &user, &password, &dbname).await.unwrap();
        engine.execute("DROP TABLE IF EXISTS test_intro").await.unwrap();
        engine.execute("CREATE TABLE test_intro (id INTEGER PRIMARY KEY, name TEXT)").await.unwrap();
        let columns = engine.introspect("test_intro").await.unwrap();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0].name, "id");
        assert_eq!(columns[0].data_type, "integer");
        assert_eq!(columns[0].not_null, true);
        assert_eq!(columns[0].default_value, None);
        assert_eq!(columns[0].primary_key, true);
        assert_eq!(columns[1].name, "name");
        assert_eq!(columns[1].data_type, "text");
        assert_eq!(columns[1].not_null, false);
        assert_eq!(columns[1].default_value, None);
        assert_eq!(columns[1].primary_key, false);
        // Cleanup
        engine.execute("DROP TABLE IF EXISTS test_intro").await.unwrap();
        drop(engine);
    }
}
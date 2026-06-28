use std::path;

use duckdb::{Connection, Result};

pub struct DuckDBEngine {
    conn: Connection,
}

impl DuckDBEngine {
    pub fn new(path: &path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Ok(DuckDBEngine { conn })
    }
}

struct DuckDBColumn {
    name: String,
    data_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key: bool,
}


trait DuckDBEngineExt { 
    fn introspect(&self, table: &str) -> Result<Vec<DuckDBColumn>>;
    fn execute(&self, sql: &str) -> Result<()>;
}

impl DuckDBEngineExt for DuckDBEngine {
    fn introspect(&self, table: &str) -> Result<Vec<DuckDBColumn>> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?;
        let mut columns: Vec<DuckDBColumn> = Vec::new();
        for column in rows {
            let (name, data_type, not_null, default_value, primary_key): (String, String, bool, Option<String>, bool) = column?;
            columns.push(DuckDBColumn {
                name,
                data_type,
                not_null,
                default_value,
                primary_key,
            });
        }
        Ok(columns)
    }

    fn execute(&self, sql: &str) -> Result<()> {
        self.conn.execute(sql, [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection() {
        let dir = std::env::temp_dir().join("vrtb_test");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test_open.db");
        println!("Database path: {:?}", db_path);
        let engine = DuckDBEngine::new(&db_path).unwrap();
        // Check that we can interact with a table
        engine.execute("CREATE TABLE IF NOT EXISTS test (id INTEGER)").unwrap();
        engine.execute("DELETE FROM test").unwrap();
        engine.execute("INSERT INTO test (id) VALUES (1)").unwrap();
        // Check that we can execute a simple query
        let val: i32 = engine.conn.query_row("SELECT * FROM test", [], |row| row.get(0)).unwrap();
        assert_eq!(val, 1);
        // Cleanup
        drop(engine);
        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_introspect() {
        let dir = std::env::temp_dir().join("vrtb_test");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("test_introspect.db");
        println!("Database path: {:?}", db_path);
        let engine = DuckDBEngine::new(&db_path).unwrap();
        engine.execute("CREATE TABLE IF NOT EXISTS test (id INTEGER, name VARCHAR PRIMARY KEY)").unwrap();
        let columns = engine.introspect("test").unwrap();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0].name, "id");
        assert_eq!(columns[0].data_type, "INTEGER");
        assert_eq!(columns[0].not_null, false);
        assert_eq!(columns[0].default_value, None);
        assert_eq!(columns[0].primary_key, false);
        assert_eq!(columns[1].name, "name");
        assert_eq!(columns[1].data_type, "VARCHAR");
        assert_eq!(columns[1].not_null, true);
        assert_eq!(columns[1].default_value, None);
        assert_eq!(columns[1].primary_key, true);
        drop(engine);
        let _ = std::fs::remove_file(&db_path);
    }
}
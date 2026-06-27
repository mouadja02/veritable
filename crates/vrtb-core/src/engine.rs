use async_trait::async_trait;

use crate::error::Result;

#[async_trait]
pub trait Engine: Send + Sync {
    async fn introspect(&self, table: &str) -> Result<TableSchema>;
    async fn execute(&self, sql: &str) -> Result<Vec<Row>>;
}

#[derive(Debug, Clone)]
pub struct TableSchema {
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub data_type: String,
}

pub type Row = Vec<Option<String>>;

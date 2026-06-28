pub mod engine;
pub mod error;

use engine::{ColumnSchema, TableSchema, TableRef, ComparePlan, DiffRow, Engine};
use error::Result;

// Resolve shared columns, fail on incomparable types
pub fn build_plan(src: &TableSchema, dst: &TableSchema, cols: Option<&[String]>, key: &str) -> Result<ComparePlan> {
    let shared_columns = match cols {
        Some(cols) => {
            let mut shared: Vec<ColumnSchema> = Vec::new();
            for col in cols {
                let src_col: Option<&ColumnSchema> = src.columns.iter().find(|c| c.name == *col);
                let dst_col: Option<&ColumnSchema> = dst.columns.iter().find(|c| c.name == *col);
                match (src_col, dst_col) {
                    (Some(src_col), Some(dst_col)) => {
                        if src_col.ty != dst_col.ty {
                            return Err(error::VeritableError::Schema(format!("Column {} has different types in source and destination", col)));
                        }
                        shared.push(src_col.clone());
                    },
                    _ => return Err(error::VeritableError::Schema(format!("Column {} not found in both source and destination", col))),
                }
            }
            shared
        },
        None => Vec::new(),
    };
    // Find the key column
    let key_column: &ColumnSchema = src.columns.iter().find(|c| c.name == key).ok_or_else(|| error::VeritableError::Schema(format!("Key column {} not found in source", key)))?;
    Ok(ComparePlan {
        key: key_column.clone(),
        columns: shared_columns,
    })
}

pub fn joindiff(engine: &dyn Engine, a: &TableRef, b: &TableRef, plan: &ComparePlan) -> Result<Vec<DiffRow>> {
    todo!()
}

pub fn hashdiff(src: &dyn Engine, dst: &dyn Engine, src_t: &TableRef, dst_t: &TableRef, plan: &ComparePlan, segment_size: usize, leaf_threshold: usize) -> Result<Vec<DiffRow>> {
    todo!()
}
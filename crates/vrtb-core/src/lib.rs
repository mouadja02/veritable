pub mod conformance;
pub mod engine;
pub mod error;
pub mod format;

use engine::{ColumnSchema, ComparePlan, DiffRow, Engine, TableRef, TableSchema};
use error::Result;

// Resolve shared columns, fail on incomparable types
pub fn build_plan(
    src: &TableSchema,
    dst: &TableSchema,
    cols: Option<&[String]>,
    key: &str,
) -> Result<ComparePlan> {
    let shared_columns = match cols {
        Some(cols) => {
            let mut shared: Vec<ColumnSchema> = Vec::new();
            for col in cols {
                let src_col: Option<&ColumnSchema> = src.columns.iter().find(|c| c.name == *col);
                let dst_col: Option<&ColumnSchema> = dst.columns.iter().find(|c| c.name == *col);
                match (src_col, dst_col) {
                    (Some(src_col), Some(dst_col)) => {
                        if src_col.ty != dst_col.ty {
                            return Err(error::VeritableError::Schema(format!(
                                "Column {} has different types in source and destination",
                                col
                            )));
                        }
                        shared.push(src_col.clone());
                    }
                    _ => {
                        return Err(error::VeritableError::Schema(format!(
                            "Column {} not found in both source and destination",
                            col
                        )));
                    }
                }
            }
            shared
        }
        None => Vec::new(),
    };
    // Find the key column
    let key_column: &ColumnSchema =
        src.columns.iter().find(|c| c.name == key).ok_or_else(|| {
            error::VeritableError::Schema(format!("Key column {} not found in source", key))
        })?;
    Ok(ComparePlan {
        key: key_column.clone(),
        columns: shared_columns,
    })
}

// Introspect both sides and plan the key plus every shared non-key column.
pub fn plan(
    src: &dyn Engine,
    src_t: &TableRef,
    dst: &dyn Engine,
    dst_t: &TableRef,
    key: &str,
) -> ComparePlan {
    let src_schema = src.introspect(src_t).unwrap();
    let dst_schema = dst.introspect(dst_t).unwrap();
    let cols: Vec<String> = src_schema
        .columns
        .iter()
        .filter(|c| c.name != key)
        .map(|c| c.name.clone())
        .collect();
    build_plan(&src_schema, &dst_schema, Some(&cols), key).unwrap()
}

// Not yet implemented — see docs/STATUS.md §5. Signatures are the intended API.
pub fn joindiff(
    _engine: &dyn Engine,
    _a: &TableRef,
    _b: &TableRef,
    _plan: &ComparePlan,
) -> Result<Vec<DiffRow>> {
    todo!()
}

pub fn hashdiff(
    _src: &dyn Engine,
    _dst: &dyn Engine,
    _src_t: &TableRef,
    _dst_t: &TableRef,
    _plan: &ComparePlan,
    _segment_size: usize,
    _leaf_threshold: usize,
) -> Result<Vec<DiffRow>> {
    todo!()
}

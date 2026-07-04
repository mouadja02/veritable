pub mod conformance;
pub mod engine;
pub mod error;
pub mod format;

use engine::{ColumnSchema, ComparePlan, DiffRow, Engine, TableRef, TableSchema};
use error::Result;

/// Resolve shared columns, fail on incomparable types. With `cols = None`,
/// defaults to every non-key column of `src` (each must also exist in `dst`).
pub fn build_plan(
    src: &TableSchema,
    dst: &TableSchema,
    cols: Option<&[String]>,
    key: &str,
) -> Result<ComparePlan> {
    // Default: every non-key column from src
    let col_names: Vec<String> = match cols {
        Some(cols) => cols.to_vec(),
        None => src
            .columns
            .iter()
            .filter(|c| c.name != key)
            .map(|c| c.name.clone())
            .collect(),
    };

    let mut shared_columns = Vec::new();
    for col in &col_names {
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
                shared_columns.push(src_col.clone());
            }
            _ => {
                return Err(error::VeritableError::Schema(format!(
                    "Column {} not found in both source and destination",
                    col
                )));
            }
        }
    }

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

/// Introspect both sides and plan the key plus every shared non-key column.
pub fn plan(
    src: &dyn Engine,
    src_t: &TableRef,
    dst: &dyn Engine,
    dst_t: &TableRef,
    key: &str,
) -> Result<ComparePlan> {
    let src_schema = src.introspect(src_t)?;
    let dst_schema = dst.introspect(dst_t)?;
    build_plan(&src_schema, &dst_schema, None, key)
}

/// Not yet implemented — see docs/STATUS.md §5. Signatures are the intended API.
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

#[cfg(test)]
mod tests {
    use super::*;
    use engine::LogicalType;

    fn col(name: &str, ty: LogicalType) -> ColumnSchema {
        ColumnSchema {
            name: name.into(),
            ty,
            nullable: true,
            default_value: None,
            primary_key: false,
        }
    }

    #[test]
    fn none_columns_defaults_to_all_non_key() {
        let src = TableSchema {
            columns: vec![
                col("id", LogicalType::Int),
                col("a", LogicalType::String),
                col("b", LogicalType::Boolean),
            ],
        };
        let dst = TableSchema {
            columns: vec![
                col("id", LogicalType::Int),
                col("a", LogicalType::String),
                col("b", LogicalType::Boolean),
            ],
        };
        let p = build_plan(&src, &dst, None, "id").unwrap();
        assert_eq!(p.key.name, "id");
        let names: Vec<&str> = p.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn default_column_missing_in_dst_is_error() {
        let src = TableSchema {
            columns: vec![col("id", LogicalType::Int), col("a", LogicalType::String)],
        };
        let dst = TableSchema {
            columns: vec![col("id", LogicalType::Int)],
        };
        assert!(build_plan(&src, &dst, None, "id").is_err());
    }

    #[test]
    fn mismatched_types_are_an_error() {
        let src = TableSchema {
            columns: vec![col("id", LogicalType::Int), col("a", LogicalType::String)],
        };
        let dst = TableSchema {
            columns: vec![col("id", LogicalType::Int), col("a", LogicalType::Boolean)],
        };
        assert!(build_plan(&src, &dst, None, "id").is_err());
    }
}

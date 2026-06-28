struct PgDialect;

// sgared, engine-agnostic the universal n/v wrapper
fn wrap(col: &ColumnSchema, payload: String) -> String {
    format!("CASE WHEN {} IS NULL THEN 'n' ELSE 'v' || {} END", col.name, payload)
}

fn canonical_expr(col: &ColumnSchema) -> Result<String> {
    let expr = match col.ty {
        LogicalType::Int => format!("CAST({} AS VARCHAR)", col.name),
        LogicalType::Decimal { scale } => format!("CAST(ROUND(CAST({} AS NUMERIC), {}) AS VARCHAR)", col.name, scale),
        LogicalType::Timestamp { precision } => {
            let p = precision.min(6);
            format!("TO_CHAR({}, 'YYYY-MM-DD HH24:MI:SS.US')", col.name)
        }
        LogicalType::String => format!("CAST({} AS VARCHAR)", col.name),
        LogicalType::Boolean => format!("CASE WHEN {} THEN '1' ELSE '0' END", col.name),
        LogicalType::Binary => format!("ENCODE({})", col.name),
        LogicalType::Float => format!("CAST({} AS VARCHAR)", col.name),
        LogicalType::Json => format!("CAST({} AS VARCHAR)", col.name),
        _ => return Err(error::VeritableError::Schema(format!("Unsupported column type for canonical expression: {:?}", col.ty))),
    };
    Ok(expr)
}
impl Dialect for PgDialect {
    // joindiff: fast-exit precheck (whole-table checksum + count)
    fn whole_table_checksum_sql(&self, table: &TableRef, plan: &ComparePlan) -> String {

    }

    // joindiff: full outer join
    fn joindiff_sql(&self, a: &TableRef, b: &TableRef, plan: &ComparePlan) -> String;

    // hashdiff: normalization matrix - One column -> canonical SQL expression
    fn normalize_column(&self, col: &ColumnSchema) -> Result<String>{
        todo!()
    }

    // hashdiff: per-row digest from canonical expressions -> md5 -> two u64 halves
    fn digest_expr(&self, canon_cols: &[String]) -> String {
        todo!()
    }
    
    // hashdiff: bound the keyspace
    fn keyspace_bounds_sql(&self, table: &TableRef, key: &ColumnSchema) -> String {
        todo!()
    }

    // hashdiff: one segment's checksum tuple, server-side execution
    fn segment_checksum_sql(&self, table: &TableRef, plan: &ComparePlan, segment: &Segment) -> String {
        todo!()
    }

    // hashdiff: leaf rows for a narrowed, still-disagreeing segment
    fn segment_rows_sql(&self, table: &TableRef, plan: &ComparePlan) -> String {
        todo!()
    }

}

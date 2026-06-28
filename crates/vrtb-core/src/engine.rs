use async_trait::async_trait;

use crate::error::Result;

pub struct TableRef {
    schema: Option<String>,
    name: String,
}

#[derive(Clone, PartialEq, Eq)]
pub enum LogicalType {
    Int,
    Decimal{scale:u8},
    Timestamp{precision:u8},
    String,
    Boolean,
    Binary,
    Float,
    Json,
}

#[derive(Clone)]
pub struct ColumnSchema {
    pub name: String,
    pub ty: LogicalType,
    pub nullable: bool,
    pub default_value: Option<String>,
    pub primary_key: bool,
}

pub struct TableSchema {
    pub columns: Vec<ColumnSchema>,
}


#[derive(Clone)]
pub struct ComparePlan {
    pub key: ColumnSchema,
    pub columns: Vec<ColumnSchema>,
}  // resolved shared cols


pub enum KeyValue {
    Int(i128),
    Bytes(Vec<u8>)
}   // int or UUID-as-bytes

pub struct Segment {
    lo: KeyValue,
    hi: KeyValue,
} // half-open [lo, hi)

pub struct Checksum {
    half1: u64,
    half2: u64,
    count: u64,
}  // 128-bit checksum + count of rows

pub enum DiffOp {
    Left,
    Right,
    Differ,
}  // Equivalent to classic diff '-', '+', '~'

pub struct DiffRow {
    op: DiffOp,
    key: KeyValue,
    columns: Vec<Option<String>>,
}

pub trait Engine {
    fn introspect(&self, table: &TableRef) -> Result<TableSchema>;
    fn dialect(&self) -> &dyn Dialect;
    fn execute(&self, sql: &str) -> Result<Vec<Vec<String>>>;
}

pub trait Dialect {
    // joindiff: fast-exit precheck (whole-table checksum + count)
    fn whole_table_checksum_sql(&self, table: &TableRef, plan: &ComparePlan) -> String;

    // joindiff: full outer join
    fn joindiff_sql(&self, a: &TableRef, b: &TableRef, plan: &ComparePlan) -> String;

    // hashdiff: normalization matrix - One column -> canonical SQL expression
    fn normalize_column(&self, col: &ColumnSchema) -> Result<String>;

    // hashdiff: per-row digest from canonical expressions -> md5 -> two u64 halves
    fn digest_expr(&self, canon_cols: &[String]) -> String;

    // hashdiff: bound the keyspace
    fn keyspace_bounds_sql(&self, table: &TableRef, key: &ColumnSchema) -> String;

    // hashdiff: one segment's checksum tuple, server-side execution
    fn segment_checksum_sql(&self, table: &TableRef, plan: &ComparePlan, segment: &Segment) -> String;

    // hashdiff: leaf rows for a narrowed, still-disagreeing segment
    fn segment_rows_sql(&self, table: &TableRef, plan: &ComparePlan) -> String;
}
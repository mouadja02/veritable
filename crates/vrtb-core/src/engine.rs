use crate::error::Result;

#[derive(Clone, Debug)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LogicalType {
    Int,
    Decimal { scale: u8 },
    Timestamp { precision: u8 },
    String,
    Boolean,
    Binary,
}

#[derive(Clone, Debug)]
pub struct ColumnSchema {
    pub name: String,
    pub ty: LogicalType,
    pub nullable: bool,
    pub default_value: Option<String>,
    pub primary_key: bool,
}

#[derive(Debug)]
pub struct TableSchema {
    pub columns: Vec<ColumnSchema>,
}

#[derive(Clone, Debug)]
pub struct ComparePlan {
    pub key: ColumnSchema,
    pub columns: Vec<ColumnSchema>,
} // resolved shared cols

#[derive(Debug)]
pub enum KeyValue {
    Int(i128),
    Bytes(Vec<u8>),
} // int or UUID-as-bytes

// Segment / Checksum / DiffRow are forward-declared scaffolding for the
// not-yet-implemented hashdiff/joindiff paths (docs/STATUS.md §5); their fields
// aren't read yet.
#[allow(dead_code)]
pub struct Segment {
    lo: KeyValue,
    hi: KeyValue,
} // half-open [lo, hi)

#[allow(dead_code)]
pub struct Checksum {
    half1: u64,
    half2: u64,
    count: u64,
} // 128-bit checksum + count of rows

#[derive(Debug)]
pub enum DiffOp {
    Left,
    Right,
    Differ,
} // Equivalent to classic diff '-', '+', '~'

#[allow(dead_code)]
pub struct DiffRow {
    op: DiffOp,
    key: KeyValue,
    columns: Vec<Option<String>>,
}

#[derive(Debug)]
pub struct JoinDiffQuery {
    pub left_only: String,
    pub right_only: String,
    pub differing: String,
}

pub trait Engine {
    fn name(&self) -> &str;
    fn introspect(&self, table: &TableRef) -> Result<TableSchema>;
    fn dialect(&self) -> &dyn Dialect;
    fn execute(&self, sql: &str) -> Result<Vec<Vec<String>>>;
}

pub trait Dialect {
    /// joindiff: fast-exit precheck (whole-table checksum + count)
    fn whole_table_checksum_sql(&self, table: &TableRef, plan: &ComparePlan) -> Result<String>;

    /// joindiff: full outer join
    fn joindiff_sql(&self, a: &TableRef, b: &TableRef, plan: &ComparePlan) -> Result<JoinDiffQuery>;

    /// materialize: one CREATE TABLE <target> AS … writing the joindiff result
    /// (op / key / src_row / dst_row JSON) server-side. Plain CTAS: fails if
    /// `target` exists — never drops or replaces (docs/superpowers spec, 2026-07-05).
    fn materialize_sql(
        &self,
        a: &TableRef,
        b: &TableRef,
        plan: &ComparePlan,
        target: &TableRef,
    ) -> Result<String>;

    /// hashdiff: normalization matrix - One column -> canonical SQL expression
    fn normalize_column(&self, col: &ColumnSchema) -> Result<String>;

    /// hashdiff: per-row digest from canonical expressions -> md5 -> two u64 halves
    fn digest_expr(&self, canon_cols: &[String]) -> Result<String>;

    /// hashdiff: bound the keyspace
    fn keyspace_bounds_sql(&self, table: &TableRef, key: &ColumnSchema) -> Result<String>;

    /// hashdiff: one segment's checksum tuple, server-side execution
    fn segment_checksum_sql(
        &self,
        table: &TableRef,
        plan: &ComparePlan,
        segment: &Segment,
    ) -> Result<String>;

    /// hashdiff: leaf rows for a narrowed, still-disagreeing segment
    fn segment_rows_sql(&self, table: &TableRef, plan: &ComparePlan) -> Result<String>;
}

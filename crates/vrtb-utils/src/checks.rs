use crate::sql::from_table;
use vrtb_core::engine::TableRef;

/// 2^64 as text — both engines carry the modular sum in a type wide
/// enough not to overflow (PG NUMERIC, DuckDB HUGEINT).
pub const TWO_POW_64: &str = "18446744073709551616";

/// The shared checksum SELECT: COUNT(*) + the two mod-2^64 half-sums.
/// `h1`/`h2` are the dialect-specific unsigned-half expressions.
pub fn checksum_select(table: &TableRef, h1: &str, h2: &str) -> String {
    format!(
        "SELECT \
           COUNT(*) AS cnt, \
           CAST(COALESCE(SUM({h1}), 0) % {m} AS VARCHAR) AS sum_h1, \
           CAST(COALESCE(SUM({h2}), 0) % {m} AS VARCHAR) AS sum_h2 \
         FROM {from}",
        m = TWO_POW_64,
        from = from_table(table),
    )
}

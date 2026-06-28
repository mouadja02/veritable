//! Engine-agnostic conformance checking.
//!
//! This is the reusable logic the integration tests used to inline: run a
//! dialect's whole-table checksum SQL through an [`Engine`], parse the single
//! result row, and compare two sides. Because it speaks only the [`Engine`] /
//! [`Dialect`] traits, it works for same-engine *and* cross-engine comparisons
//! and carries no database-driver dependencies — the concrete engines live in
//! the adapter crates (`vrtb-pg`, `vrtb-duck`), and the live tests that exercise
//! real databases live in the CLI crate.

use crate::engine::{ComparePlan, Engine, TableRef};
use crate::error::{Result, VeritableError};

/// The fast-exit precheck tuple: row count plus the two 64-bit checksum halves
/// (kept as decimal strings — they are unsigned 64-bit values that may exceed
/// `i64`, and the engines already hand them back as text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksumResult {
    pub count: u64,
    pub sum_h1: String,
    pub sum_h2: String,
}

/// Outcome of comparing two sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Counts and both checksum halves matched — the tables are equal under the
    /// joindiff fast-exit precheck.
    Match,
    /// At least one of count / h1 / h2 differed.
    Differ {
        src: ChecksumResult,
        dst: ChecksumResult,
    },
}

impl Verdict {
    pub fn is_match(&self) -> bool {
        matches!(self, Verdict::Match)
    }
}

/// Build the dialect's whole-table checksum SQL, run it on `engine`, and parse
/// the single `(cnt, sum_h1, sum_h2)` row it returns.
pub fn whole_table_checksum(
    engine: &dyn Engine,
    table: &TableRef,
    plan: &ComparePlan,
) -> Result<ChecksumResult> {
    let sql = engine.dialect().whole_table_checksum_sql(table, plan)?;
    let rows = engine.execute(&sql)?;
    parse_checksum_row(rows)
}

/// Checksum both sides (which may be backed by different engines) and compare.
pub fn conformance_check(
    src: &dyn Engine,
    src_table: &TableRef,
    dst: &dyn Engine,
    dst_table: &TableRef,
    plan: &ComparePlan,
) -> Result<Verdict> {
    let s = whole_table_checksum(src, src_table, plan)?;
    let d = whole_table_checksum(dst, dst_table, plan)?;
    Ok(if s == d {
        Verdict::Match
    } else {
        Verdict::Differ { src: s, dst: d }
    })
}

/// Pull the single checksum row out of an engine result set. The checksum SQL
/// always selects exactly `cnt, sum_h1, sum_h2` in that order.
fn parse_checksum_row(rows: Vec<Vec<String>>) -> Result<ChecksumResult> {
    let row = rows.into_iter().next().ok_or_else(|| {
        VeritableError::Query("checksum query returned no rows".into())
    })?;
    if row.len() < 3 {
        return Err(VeritableError::Query(format!(
            "checksum query returned {} columns, expected 3 (cnt, sum_h1, sum_h2)",
            row.len()
        )));
    }
    let count = row[0].parse::<u64>().map_err(|e| {
        VeritableError::Query(format!("could not parse row count {:?}: {e}", row[0]))
    })?;
    Ok(ChecksumResult {
        count,
        sum_h1: row[1].clone(),
        sum_h2: row[2].clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{
        ColumnSchema, Dialect, LogicalType, Segment, TableSchema,
    };

    // A dialect whose checksum SQL is a sentinel string; the mock engine ignores
    // the SQL text and returns whatever rows it was seeded with.
    struct MockDialect;
    impl Dialect for MockDialect {
        fn whole_table_checksum_sql(&self, t: &TableRef, _p: &ComparePlan) -> Result<String> {
            Ok(format!("CHECKSUM {}", t.name))
        }
        fn joindiff_sql(&self, _a: &TableRef, _b: &TableRef, _p: &ComparePlan) -> Result<String> {
            unimplemented!()
        }
        fn normalize_column(&self, _c: &ColumnSchema) -> Result<String> {
            unimplemented!()
        }
        fn digest_expr(&self, _c: &[String]) -> Result<String> {
            unimplemented!()
        }
        fn keyspace_bounds_sql(&self, _t: &TableRef, _k: &ColumnSchema) -> Result<String> {
            unimplemented!()
        }
        fn segment_checksum_sql(
            &self,
            _t: &TableRef,
            _p: &ComparePlan,
            _s: &Segment,
        ) -> Result<String> {
            unimplemented!()
        }
        fn segment_rows_sql(&self, _t: &TableRef, _p: &ComparePlan) -> Result<String> {
            unimplemented!()
        }
    }

    struct MockEngine {
        dialect: MockDialect,
        rows: Vec<Vec<String>>,
    }
    impl MockEngine {
        fn new(rows: Vec<Vec<String>>) -> Self {
            MockEngine { dialect: MockDialect, rows }
        }
    }
    impl Engine for MockEngine {
        fn introspect(&self, _t: &TableRef) -> Result<TableSchema> {
            unimplemented!()
        }
        fn dialect(&self) -> &dyn Dialect {
            &self.dialect
        }
        fn execute(&self, _sql: &str) -> Result<Vec<Vec<String>>> {
            Ok(self.rows.clone())
        }
    }

    fn row(cnt: &str, h1: &str, h2: &str) -> Vec<String> {
        vec![cnt.into(), h1.into(), h2.into()]
    }

    fn plan() -> ComparePlan {
        ComparePlan {
            key: ColumnSchema {
                name: "id".into(),
                ty: LogicalType::Int,
                nullable: false,
                default_value: None,
                primary_key: true,
            },
            columns: vec![],
        }
    }

    fn tref(name: &str) -> TableRef {
        TableRef { schema: None, name: name.into() }
    }

    #[test]
    fn parses_checksum_row() {
        let eng = MockEngine::new(vec![row("10000", "123", "456")]);
        let r = whole_table_checksum(&eng, &tref("t"), &plan()).unwrap();
        assert_eq!(
            r,
            ChecksumResult { count: 10000, sum_h1: "123".into(), sum_h2: "456".into() }
        );
    }

    #[test]
    fn identical_sides_match() {
        let a = MockEngine::new(vec![row("5", "aaa", "bbb")]);
        let b = MockEngine::new(vec![row("5", "aaa", "bbb")]);
        let v = conformance_check(&a, &tref("t"), &b, &tref("t"), &plan()).unwrap();
        assert!(v.is_match());
    }

    #[test]
    fn differing_checksum_differs() {
        let a = MockEngine::new(vec![row("5", "aaa", "bbb")]);
        let b = MockEngine::new(vec![row("5", "aaa", "ZZZ")]);
        let v = conformance_check(&a, &tref("t"), &b, &tref("t"), &plan()).unwrap();
        match v {
            Verdict::Differ { src, dst } => {
                assert_eq!(src.sum_h2, "bbb");
                assert_eq!(dst.sum_h2, "ZZZ");
            }
            Verdict::Match => panic!("expected Differ"),
        }
    }

    #[test]
    fn differing_count_differs() {
        let a = MockEngine::new(vec![row("5", "aaa", "bbb")]);
        let b = MockEngine::new(vec![row("6", "aaa", "bbb")]);
        let v = conformance_check(&a, &tref("t"), &b, &tref("t"), &plan()).unwrap();
        assert!(!v.is_match());
    }

    #[test]
    fn empty_result_is_error() {
        let eng = MockEngine::new(vec![]);
        assert!(whole_table_checksum(&eng, &tref("t"), &plan()).is_err());
    }

    #[test]
    fn too_few_columns_is_error() {
        let eng = MockEngine::new(vec![vec!["5".into(), "aaa".into()]]);
        assert!(whole_table_checksum(&eng, &tref("t"), &plan()).is_err());
    }
}

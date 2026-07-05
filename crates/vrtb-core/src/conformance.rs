//! Engine-agnostic conformance checking.
//!
//! This is the reusable logic the integration tests used to inline: run a
//! dialect's whole-table checksum SQL through an [`Engine`], parse the single
//! result row, and compare two sides. Because it speaks only the [`Engine`] /
//! [`Dialect`] traits, it works for same-engine *and* cross-engine comparisons
//! and carries no database-driver dependencies — the concrete engines live in
//! the adapter crates (`vrtb-pg`, `vrtb-duck`), and the live tests that exercise
//! real databases live in the CLI crate.

use crate::engine::{ComparePlan, Engine, JoinDiffQuery, TableRef};
use crate::error::{Result, VeritableError};
use crate::format::Format;

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

// Render the three joindiff row groups in the requested format. Following the
// classic diff convention (see `DiffOp`): `src_only` rows are left-only (`-`),
// `dst_only` rows are right-only (`+`), and `differing` rows share a key but
// disagree on non-key columns (`~`).
fn output_diff(
    src_only: &[Vec<String>],
    dst_only: &[Vec<String>],
    differing: &[Vec<String>],
    format: Format,
) -> Result<()> {
    match format {
        Format::Human => {
            println!("DIFFER — row-level differences");
            println!(
                "  {} only in src, {} only in dst, {} differing",
                src_only.len(),
                dst_only.len(),
                differing.len()
            );
            for row in src_only {
                println!("  - {}", row.join(" | "));
            }
            for row in dst_only {
                println!("  + {}", row.join(" | "));
            }
            for row in differing {
                println!("  ~ {}", row.join(" | "));
            }
        }
        Format::Summary => {
            println!(
                "differ: {} only in src, {} only in dst, {} differing",
                src_only.len(),
                dst_only.len(),
                differing.len()
            );
        }
        Format::Json => {
            println!(
                r#"{{"result":"differ","src_only":{},"dst_only":{},"differing":{}}}"#,
                json_rows(src_only),
                json_rows(dst_only),
                json_rows(differing)
            );
        }
        Format::Jsonl => {
            for (op, rows) in [
                ("src_only", src_only),
                ("dst_only", dst_only),
                ("differing", differing),
            ] {
                for row in rows {
                    println!(r#"{{"op":"{}","row":{}}}"#, op, json_row(row));
                }
            }
        }
    }
    Ok(())
}

// Minimal JSON rendering — core carries no serde dependency (see `verdict_json`
// in the CLI, which hand-rolls output the same way).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn json_row(row: &[String]) -> String {
    let cells: Vec<String> = row
        .iter()
        .map(|c| format!("\"{}\"", json_escape(c)))
        .collect();
    format!("[{}]", cells.join(","))
}

fn json_rows(rows: &[Vec<String>]) -> String {
    let items: Vec<String> = rows.iter().map(|r| json_row(r)).collect();
    format!("[{}]", items.join(","))
}

// Server-side materialization of the joindiff result. Implemented in a later
// task; the signature is final. See docs/superpowers/specs/2026-07-05-materialize-design.md.
fn materialize_diff(
    _engine: &dyn Engine,
    _a: &TableRef,
    _b: &TableRef,
    _plan: &ComparePlan,
    _target: &TableRef,
    _format: Format,
) -> Result<()> {
    Err(VeritableError::Engine(
        "--materialize is not implemented yet".into(),
    ))
}

/// Checksum both sides (which may be backed by different engines) and compare.
/// On a same-engine differ: stream differing keys (default) or, with
/// `materialize`, write the full diff server-side into that table instead.
pub fn conformance_check(
    src: &dyn Engine,
    src_table: &TableRef,
    dst: &dyn Engine,
    dst_table: &TableRef,
    plan: &ComparePlan,
    format: Format,
    materialize: Option<&TableRef>,
) -> Result<Verdict> {
    let s = whole_table_checksum(src, src_table, plan)?;
    let d = whole_table_checksum(dst, dst_table, plan)?;
    Ok(if s == d {
        if materialize.is_some() && matches!(format, Format::Human | Format::Summary) {
            println!("nothing to materialize — tables already match");
        }
        Verdict::Match
    } else {
        // Row-level joindiff needs both tables on one connection.
        if src.name() == dst.name() {
            match materialize {
                Some(target) => {
                    materialize_diff(src, src_table, dst_table, plan, target, format)?;
                }
                None => {
                    let sql: JoinDiffQuery =
                        src.dialect().joindiff_sql(src_table, dst_table, plan)?;
                    let src_only_rows = src.execute(&sql.left_only)?;
                    let dst_only_rows = src.execute(&sql.right_only)?;
                    let diff_rows = src.execute(&sql.differing)?;
                    output_diff(&src_only_rows, &dst_only_rows, &diff_rows, format)?;
                }
            }
        } else {
            // Defense in depth — the CLI rejects this before connecting.
            if materialize.is_some() {
                return Err(VeritableError::Config(
                    "--materialize requires src and dst on the same connection (joindiff); \
                     cross-engine materialize arrives with hashdiff"
                        .into(),
                ));
            }
            // Cross-engine conformance check: engines disagree on checksums
            // but we cannot run a join-diff across engines. Just report the mismatch.
            eprintln!(
                "Cross-engine conformance check: engines disagree on checksums, but cannot run join-diff across engines."
            );
            // We will suggest running HashDiff for cross-engine conformance check
            eprintln!("Suggestion: Run HashDiff for cross-engine conformance check.");
        }
        Verdict::Differ { src: s, dst: d }
    })
}

// Pull the single checksum row out of an engine result set. The checksum SQL
// always selects exactly `cnt, sum_h1, sum_h2` in that order.
fn parse_checksum_row(rows: Vec<Vec<String>>) -> Result<ChecksumResult> {
    let row = rows
        .into_iter()
        .next()
        .ok_or_else(|| VeritableError::Query("checksum query returned no rows".into()))?;
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
    use crate::engine::{ColumnSchema, Dialect, JoinDiffQuery, LogicalType, Segment, TableSchema};
    use crate::format::Format;

    // A dialect whose checksum SQL is a sentinel string; the mock engine ignores
    // the SQL text and returns whatever rows it was seeded with.
    struct MockDialect;
    impl Dialect for MockDialect {
        fn whole_table_checksum_sql(&self, t: &TableRef, _p: &ComparePlan) -> Result<String> {
            Ok(format!("CHECKSUM {}", t.name))
        }
        fn joindiff_sql(
            &self,
            _a: &TableRef,
            _b: &TableRef,
            _p: &ComparePlan,
        ) -> Result<JoinDiffQuery> {
            Ok(JoinDiffQuery {
                left_only: "LEFT".into(),
                right_only: "RIGHT".into(),
                differing: "DIFF".into(),
            })
        }
        fn materialize_sql(
            &self,
            _a: &TableRef,
            _b: &TableRef,
            _p: &ComparePlan,
            target: &TableRef,
        ) -> Result<String> {
            Ok(format!("CTAS {}", target.name))
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
            MockEngine {
                dialect: MockDialect,
                rows,
            }
        }
    }
    impl Engine for MockEngine {
        fn name(&self) -> &str {
            "mock"
        }
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
        TableRef {
            schema: None,
            name: name.into(),
        }
    }

    #[test]
    fn parses_checksum_row() {
        let eng = MockEngine::new(vec![row("10000", "123", "456")]);
        let r = whole_table_checksum(&eng, &tref("t"), &plan()).unwrap();
        assert_eq!(
            r,
            ChecksumResult {
                count: 10000,
                sum_h1: "123".into(),
                sum_h2: "456".into()
            }
        );
    }

    #[test]
    fn identical_sides_match() {
        let a = MockEngine::new(vec![row("5", "aaa", "bbb")]);
        let b = MockEngine::new(vec![row("5", "aaa", "bbb")]);
        let v =
            conformance_check(&a, &tref("t"), &b, &tref("t"), &plan(), Format::Summary, None).unwrap();
        assert!(v.is_match());
    }

    #[test]
    fn differing_checksum_differs() {
        let a = MockEngine::new(vec![row("5", "aaa", "bbb")]);
        let b = MockEngine::new(vec![row("5", "aaa", "ZZZ")]);
        let v =
            conformance_check(&a, &tref("t"), &b, &tref("t"), &plan(), Format::Summary, None).unwrap();
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
        let v =
            conformance_check(&a, &tref("t"), &b, &tref("t"), &plan(), Format::Summary, None).unwrap();
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

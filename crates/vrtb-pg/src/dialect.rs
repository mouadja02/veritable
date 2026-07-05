use vrtb_core::engine::{
    ColumnSchema, ComparePlan, Dialect, JoinDiffQuery, LogicalType, Segment, TableRef,
};
use vrtb_core::error::Result;
use vrtb_utils::checks::{TWO_POW_64, checksum_select};
use vrtb_utils::sql::{
    aliased_key, from_table, json_object_args, mismatch_condition, outer_join_from, quote_ident,
    wrap,
};

pub struct PgDialect;

// postgres specific canonical expression for a column, or Err if the type is unsupported.
fn canonical_expr(col: &ColumnSchema) -> Result<String> {
    let payload = match col.ty {
        // Minimal decimal text. `42` -> "v42", `-7` -> "v-7". Unambiguous: both
        // engines render integers identically, no scale or locale to negotiate.
        LogicalType::Int => format!("CAST({} AS VARCHAR)", quote_ident(&col.name)),

        // Raw UTF-8, passed through. This is the ONLY type whose payload can
        // contain the 0x1F separator (or a 'v'), so it's the only one that can
        // shift column boundaries — see the digest builder's collision note.
        LogicalType::String => format!("CAST({} AS VARCHAR)", quote_ident(&col.name)),

        // Booleans canonicalize to '1' / '0'. CAST(bool AS VARCHAR) would give
        // 'true'/'false' in Postgres but 't'/'f' elsewhere — so spell it out
        // explicitly instead of trusting each engine's text rendering.
        LogicalType::Boolean => {
            format!("CASE WHEN {} THEN '1' ELSE '0' END", quote_ident(&col.name))
        }

        // Lowercase hex. Postgres ENCODE(bytea,'hex') is already lowercase,
        // which is the canonical form — no LOWER() needed here, but DuckDB's
        // equivalent (hex()) is UPPER, so its dialect WILL need LOWER().
        LogicalType::Binary => format!("ENCODE({}, 'hex')", quote_ident(&col.name)),

        // Decimal — correct target form (fixed-point text at the *negotiated*
        // scale; trailing zeros preserved, e.g. 1.5 at scale 2 -> "1.50"):
        LogicalType::Decimal { scale } => {
            format!(
                "CAST({} AS NUMERIC(38, {}))::TEXT",
                quote_ident(&col.name),
                scale
            )
        }
        // Blocked on: `scale` must come from build_plan's negotiation, not
        // col.ty — two sides at scale 2 vs 4 is an error unless --coerce-scale.

        // Timestamp — naive wall-clock, fixed width:
        LogicalType::Timestamp { precision } => {
            // Render the stored value as text exactly as-is, always 6 fractional
            // digits. No tz conversion: these are NAIVE timestamps (no zone), so
            // the wall-clock IS the canonical value — comparing it byte-for-byte is
            // deterministic and engine-independent. (Applying AT TIME ZONE 'UTC'
            // here would interpret the naive value as UTC and yield a timestamptz,
            // which each engine then re-localizes differently — a conformance
            // hazard. UTC-normalizing genuinely tz-aware columns is a separate
            // branch, gated on --assume-tz.)
            let rendered = format!(
                "TO_CHAR({}, 'YYYY-MM-DD HH24:MI:SS.US')",
                quote_ident(&col.name)
            );

            // Reduce to precision p by TRUNCATING THE TEXT, not by casting the value.
            // Why text-slice and not CAST(... AS timestamp(p)): casting rounds, and
            // engines disagree on rounding-vs-truncation at the boundary. String
            // slicing is the ONE reduction guaranteed bit-identical on every engine —
            // which is the entire job of canonicalization. The full render is fixed
            // layout: 19 chars "YYYY-MM-DD HH24:MI:SS" + '.' + 6 digits = 26 total.
            //   precision == 6 → keep all 26
            //   precision in 1..=5 → keep 19 + 1 (dot) + precision = 20 + precision
            //   precision == 0 → drop the dot too, keep 19
            match precision {
                6 => rendered,
                0 => format!("LEFT({}, 19)", rendered),
                _ => format!("LEFT({}, {})", rendered, 20 + precision),
            }
        }
    };
    Ok(wrap(col, payload))
}

impl Dialect for PgDialect {
    // Joindiff fast-exit precheck: COUNT(*) + one order-independent whole-table
    // checksum per side. If both match across sides, the tables are identical
    // and we stop — the common case made cheap.
    fn whole_table_checksum_sql(&self, table: &TableRef, plan: &ComparePlan) -> Result<String> {
        // Include the key in the checksum as well as the compared columns, so a
        // changed key is itself a detectable difference
        let mut canon_cols: Vec<String> = vec![canonical_expr(&plan.key)?];
        for col in &plan.columns {
            canon_cols.push(canonical_expr(col)?);
        }

        // Per-row digest: concatenate canonical columns with chr(31) (0x1F, ASCII
        // unit separator) between them, then MD5. The separator keeps column
        // boundaries from sliding — without it ("ab","c") and ("a","bc") would
        // hash identically. (It's a structural aid, not injection-proof: a string
        // payload can itself contain 0x1F.
        let digest = format!("MD5({})", canon_cols.join(" || chr(31) || "));

        // ---- The fiddly part: 128-bit MD5 -> two UNSIGNED 64-bit halves. ----
        // MD5 returns 32 hex chars. Split into the top 16 and bottom 16.
        // ('x' || hex16)::bit(64)::bigint reinterprets the bits as a *signed*
        // i64 — negative whenever the high bit is set. Convert to the unsigned
        // value branchlessly with: (b + 2^64) % 2^64  (a no-op when b >= 0).
        // NUMERIC carries this without overflow; BIGINT would wrap.
        let half_unsigned = |start: i32| {
            format!(
                "(((('x' || substr({digest}, {start}, 16))::bit(64)::bigint)::numeric \
                  + {TWO_POW_64}) % {TWO_POW_64})"
            )
        };

        // Per segment/table: SUM each half, reduce mod 2^64, and carry COUNT(*).
        // Summation is what makes this order-independent — no ORDER BY, which is
        // most of the speed. Each engine MUST reproduce this arithmetic bit-for-
        // bit; the conformance suite is what enforces that. The SELECT skeleton
        // is shared with segment_checksum_sql via checks::checksum_select.
        Ok(checksum_select(
            table,
            &half_unsigned(1),
            &half_unsigned(17),
        ))
    }

    // joindiff: full outer join
    fn joindiff_sql(
        &self,
        a: &TableRef,
        b: &TableRef,
        plan: &ComparePlan,
    ) -> Result<JoinDiffQuery> {
        let join_key = quote_ident(&plan.key.name);
        let join = outer_join_from(a, b, &plan.key);
        let left_only = format!(
            "SELECT {} {} WHERE b.{} IS NULL",
            aliased_key("a", &plan.key),
            join,
            join_key
        );
        let right_only = format!(
            "SELECT {} {} WHERE a.{} IS NULL",
            aliased_key("b", &plan.key),
            join,
            join_key
        );
        let differing = format!(
            "SELECT {} {} WHERE a.{} IS NOT NULL AND b.{} IS NOT NULL AND ({})",
            aliased_key("a", &plan.key),
            join,
            join_key,
            join_key,
            mismatch_condition(plan)
        );
        Ok(JoinDiffQuery {
            left_only,
            right_only,
            differing,
        })
    }

    // materialize: one server-side CTAS, three UNION ALL branches over the same
    // join skeleton joindiff uses. `key` is selected UNqualified — after USING
    // it is the coalesced value, correct in all three branches. Values go into
    // JSONB columns built only from the plan's compared columns; they never
    // leave the database (README §0.3) — the caller reads back counts only.
    fn materialize_sql(
        &self,
        a: &TableRef,
        b: &TableRef,
        plan: &ComparePlan,
        target: &TableRef,
    ) -> Result<String> {
        let join = outer_join_from(a, b, &plan.key);
        let key = quote_ident(&plan.key.name);
        let src_json = format!("jsonb_build_object({})", json_object_args("a", &plan.columns));
        let dst_json = format!("jsonb_build_object({})", json_object_args("b", &plan.columns));
        Ok(format!(
            "CREATE TABLE {target} AS \
             SELECT '-' AS \"op\", {key} AS \"key\", {src_json} AS \"src_row\", CAST(NULL AS JSONB) AS \"dst_row\" {join} WHERE b.{key} IS NULL \
             UNION ALL SELECT '+', {key}, CAST(NULL AS JSONB), {dst_json} {join} WHERE a.{key} IS NULL \
             UNION ALL SELECT '~', {key}, {src_json}, {dst_json} {join} WHERE a.{key} IS NOT NULL AND b.{key} IS NOT NULL AND ({mismatch})",
            target = from_table(target),
            mismatch = mismatch_condition(plan),
        ))
    }

    // hashdiff: normalization matrix - One column -> canonical SQL expression
    fn normalize_column(&self, _col: &ColumnSchema) -> Result<String> {
        todo!()
    }

    // hashdiff: per-row digest from canonical expressions -> md5 -> two u64 halves
    fn digest_expr(&self, _canon_cols: &[String]) -> Result<String> {
        todo!()
    }

    // hashdiff: bound the keyspace
    fn keyspace_bounds_sql(&self, _table: &TableRef, _key: &ColumnSchema) -> Result<String> {
        todo!()
    }

    // hashdiff: one segment's checksum tuple, server-side execution
    fn segment_checksum_sql(
        &self,
        _table: &TableRef,
        _plan: &ComparePlan,
        _segment: &Segment,
    ) -> Result<String> {
        todo!()
    }

    // hashdiff: leaf rows for a narrowed, still-disagreeing segment
    fn segment_rows_sql(&self, _table: &TableRef, _plan: &ComparePlan) -> Result<String> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: LogicalType) -> ColumnSchema {
        ColumnSchema {
            name: name.into(),
            ty,
            nullable: true,
            default_value: None,
            primary_key: false,
        }
    }

    fn tref(name: &str) -> TableRef {
        TableRef { schema: None, name: name.into() }
    }

    #[test]
    fn materialize_sql_builds_three_branch_ctas() {
        let plan = ComparePlan {
            key: col("id", LogicalType::Int),
            columns: vec![col("name", LogicalType::String)],
        };
        let sql = PgDialect
            .materialize_sql(&tref("src"), &tref("dst"), &plan, &tref("report"))
            .unwrap();
        assert!(sql.starts_with("CREATE TABLE \"report\" AS SELECT "));
        assert!(sql.contains(
            "SELECT '-' AS \"op\", \"id\" AS \"key\", \
             jsonb_build_object('name', a.\"name\") AS \"src_row\", \
             CAST(NULL AS JSONB) AS \"dst_row\" \
             FROM \"src\" a FULL OUTER JOIN \"dst\" b USING (\"id\") WHERE b.\"id\" IS NULL"
        ));
        assert!(sql.contains("UNION ALL SELECT '+', \"id\", CAST(NULL AS JSONB), jsonb_build_object('name', b.\"name\")"));
        assert!(sql.contains("UNION ALL SELECT '~', \"id\", jsonb_build_object('name', a.\"name\"), jsonb_build_object('name', b.\"name\")"));
        assert!(sql.contains("WHERE a.\"id\" IS NOT NULL AND b.\"id\" IS NOT NULL AND ("));
    }
}

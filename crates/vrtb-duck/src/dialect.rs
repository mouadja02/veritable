use vrtb_core::engine::{ColumnSchema, LogicalType, TableRef, ComparePlan, Segment, Dialect};
use vrtb_core::error::Result;

pub struct DuckDBDialect;

// 2^64, as text, for the modular-sum arithmetic below. DuckDB NUMERIC handles
const TWO_POW_64: &str = "18446744073709551616";

// Shared, engine-agnostic: the universal n/v wrapper.
// NULL  -> 'n'      (presence is encoded in the very first byte)
// value -> 'v' + payload
// Every non-null starts with 'v', so no payload can ever be mistaken for the
// NULL sentinel 'n' — not the empty string, not a literal "n". The wrapper is
// identical for every type on every engine; only `payload` varies per cell.
fn wrap(col: &ColumnSchema, payload: String) -> String {
    format!("CASE WHEN {} IS NULL THEN 'n' ELSE 'v' || {} END", col.name, payload)
}

// DuckDB specific canonical expression for a column, or Err if the type is unsupported.
fn canonical_expr(col: &ColumnSchema) -> Result<String> {
    let payload = match col.ty {
        // Minimal decimal text. `42` -> "v42", `-7` -> "v-7". Unambiguous: both
        // engines render integers identically, no scale or locale to negotiate.
        LogicalType::Int => format!("CAST({} AS VARCHAR)", col.name),

        // Raw UTF-8, passed through. This is the ONLY type whose payload can
        // contain the 0x1F separator (or a 'v'), so it's the only one that can
        // shift column boundaries — see the digest builder's collision note.
        LogicalType::String => format!("CAST({} AS VARCHAR)", col.name),

        // Booleans canonicalize to '1' / '0'. CAST(bool AS VARCHAR) would give
        // 'true'/'false' in Postgres but 't'/'f' elsewhere — so spell it out
        // explicitly instead of trusting each engine's text rendering.
        LogicalType::Boolean => format!("CASE WHEN {} THEN '1' ELSE '0' END", col.name),

        // Lowercase hex. DuckDB ENCODE(bytea,'hex') is uppercase, which is
        // NOT the canonical form — so LOWER() is needed here. 
        LogicalType::Binary => format!("LOWER(ENCODE({}, 'hex'))", col.name),

        // Decimal — correct target form (fixed-point text at the *negotiated*
        // scale; trailing zeros preserved, e.g. 1.5 at scale 2 -> "1.50"):
        LogicalType::Decimal{scale} => format!(" CAST({} AS NUMERIC(38, {}))::TEXT", col.name, scale),
        // Blocked on: `scale` must come from build_plan's negotiation, not
        // col.ty — two sides at scale 2 vs 4 is an error unless --coerce-scale.
        
        
        // Timestamp — naive wall-clock, fixed width:
        LogicalType::Timestamp{precision} =>{
            // Render the stored NAIVE timestamp directly as text — no tz conversion.
            // (AT TIME ZONE 'UTC' on a naive TIMESTAMP yields a timestamptz, and
            // CAST(... AS TIMESTAMP) then re-localizes it to the session tz, shifting
            // the value — a cross-engine conformance hazard. The wall-clock itself is
            // the canonical value. UTC-normalizing tz-aware columns is future work,
            // gated on --assume-tz.)
            // CAST(TIMESTAMP AS VARCHAR) gives ISO format but may trim trailing
            // fractional zeros. Pad to a fixed 26-char layout:
            //   'YYYY-MM-DD HH:MM:SS.ffffff'
            // so both engines produce identical strings.
            let ts_text = format!(
                "CAST({} AS VARCHAR)",
                col.name
            );
            // Ensure exactly 6 fractional digits (26 chars total).
            let rendered = format!(
                "CASE WHEN POSITION('.' IN {txt}) = 0 \
                 THEN {txt} || '.000000' \
                 ELSE RPAD({txt}, 26, '0') END",
                txt = ts_text
            );

            // Reduce to precision p by TRUNCATING THE TEXT, not by casting the value.
            // Why text-slice and not CAST(... AS timestamp(p)): casting rounds, and
            // engines disagree on rounding-vs-truncation at the boundary. String
            // slicing is the ONE reduction guaranteed bit-identical on every engine —
            // which is the entire job of canonicalization. The full render is fixed
            // layout: 19 chars "YYYY-MM-DD HH24:MI:SS" + '.' + 6 digits = 26 total.
            //   p == 6 → keep all 26
            //   precision in 1..=5 → keep 19 + 1 (dot) + precision = 20 + precision
            //   precision == 0 → drop the dot too, keep 19
            match precision {
                6 => rendered,
                0 => format!("LEFT({}, 19)", rendered),
                _ => format!("LEFT({}, {})", rendered, 20 + precision),
            }
        },
    };
    Ok(wrap(col, payload))
}

impl Dialect for DuckDBDialect {
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

        // ---- 128-bit MD5 -> two UNSIGNED 64-bit halves. ----
        // MD5 returns 32 hex chars; take the top 16 (h1) and bottom 16 (h2).
        // Each 16-hex-char half is exactly 64 bits — UBIGINT's range. DuckDB casts
        // a '0x…' hex string straight to UBIGINT in one step. We do NOT split into
        // two 32-bit pieces and recombine (`hi32 * 2^32 + lo32`): that mixes BIGINT
        // and HUGEINT operands, and DuckDB 1.1.x's binder intermittently fails to
        // resolve the resulting HUGEINT arithmetic ("No function matches … '(HUGEINT,
        // HUGEINT)'") on a fresh connection. Casting straight to UBIGINT sidesteps
        // it. (HUGEINT/INT128 is NOT a valid target for a '0x…' string cast.)
        let half_unsigned = |start: i32| {
            format!("CAST(('0x' || substr({digest}, {start}, 16)) AS UBIGINT)")
        };

        // Per segment/table: SUM each half, reduce mod 2^64, and carry COUNT(*).
        // Summation is what makes this order-independent — no ORDER BY, which is
        // most of the speed. Each engine MUST reproduce this arithmetic bit-for-
        // bit; the conformance suite is what enforces that. This block is shared
        // with segment_checksum_sql — factor it into a private helper rather than
        // writing it twice.
        let from = match &table.schema {
            Some(s) => format!("{}.{}", s, table.name),
            None => table.name.clone(),
        };

        Ok(format!(
            "SELECT \
               COUNT(*) AS cnt, \
               CAST(COALESCE(SUM({h1}), 0) % {m} AS VARCHAR) AS sum_h1, \
               CAST(COALESCE(SUM({h2}), 0) % {m} AS VARCHAR) AS sum_h2 \
             FROM {from}",
            h1 = half_unsigned(1),
            h2 = half_unsigned(17),
            m = TWO_POW_64,
        ))
    }

    // joindiff: full outer join
    fn joindiff_sql(&self, _a: &TableRef, _b: &TableRef, _plan: &ComparePlan) -> Result<String> {
        todo!()
    }

    // hashdiff: normalization matrix - One column -> canonical SQL expression
    fn normalize_column(&self, _col: &ColumnSchema) -> Result<String>{
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
    fn segment_checksum_sql(&self, _table: &TableRef, _plan: &ComparePlan, _segment: &Segment) -> Result<String> {
        todo!()
    }

    // hashdiff: leaf rows for a narrowed, still-disagreeing segment
    fn segment_rows_sql(&self, _table: &TableRef, _plan: &ComparePlan) -> Result<String> {
        todo!()
    }

}

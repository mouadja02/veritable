# Veritable — Status, Decisions & Remaining Work

_Last updated: 2026-06-28_
_note: This document is AI-generated, here is the prompt "explore the veritable craates codebase and try to document the current status, decisions made and what remains to do, output in a file named STATUS.md in doc/ folder" 

Veritable (`vrtb`) is a local and cross-database result-set comparison engine.
It canonicalizes each row to a byte-identical string on every supported engine,
hashes it, and compares order-independent checksums so that the same data
produces the same checksum whether it lives in PostgreSQL or DuckDB.

This document records what was fixed, the design decisions behind the current
shape, and what is still stubbed.

---

## 1. Workspace layout

```
crates/
  vrtb-core    Pure traits + engine-agnostic logic. NO database drivers.
               - engine.rs       Engine / Dialect traits, LogicalType, ComparePlan, …
               - conformance.rs  whole_table_checksum + conformance_check (over the Engine trait)
               - error.rs        VeritableError + exit codes
               - lib.rs          build_plan, joindiff/hashdiff (stubs)
  vrtb-pg      PostgresEngine + PgDialect (feature: postgres, default on)
  vrtb-duck    DuckDBEngine + DuckDBDialect (feature: duckdb, default on)
  vrtb-cli     `veritable` binary: check / conformance / diff(stub) + live tests
```

`vrtb-integ` (a test-only crate) **was removed** — see §3.

Dependency direction: `vrtb-pg` / `vrtb-duck` / `vrtb-cli` depend on `vrtb-core`;
`vrtb-core` depends on **nothing DB-related**. This is why the live cross-engine
tests live in `vrtb-cli` (the only crate that can construct both concrete
engines) and the engine-agnostic comparison logic lives in `vrtb-core`.

---

## 2. Bugs fixed this session

### 2.1 DuckDB binder error on the checksum SQL (`HUGEINT, HUGEINT`)

**Symptom:** every DuckDB checksum query failed at *prepare* time with
`Binder Error: No function matches the given name and argument types
'(HUGEINT, HUGEINT)'`, deterministically on a fresh connection (which is exactly
what each checksum run uses).

**Root cause:** `DuckDBDialect::whole_table_checksum_sql` built the unsigned
64-bit value of each MD5 half by splitting it into two 32-bit pieces and
recombining them: `hi32::HUGEINT * 2^32 + lo32::HUGEINT`. That mixes `BIGINT`
and `HUGEINT` operands, and DuckDB 1.1.x's binder intermittently fails to
resolve the resulting `HUGEINT` arithmetic. Adding explicit `::HUGEINT` casts on
the literal did **not** help (verified).

**Fix:** cast the whole 16-hex-char half straight to `UBIGINT` —
`CAST(('0x' || substr(digest, start, 16)) AS UBIGINT)`. A 16-hex-char value is
exactly 64 bits, i.e. `UBIGINT`'s range, so no split is needed. DuckDB casts a
`'0x…'` string to `UBIGINT` happily (but **not** to `HUGEINT`/INT128). Proven
byte-identical to the old arithmetic: 0 per-row mismatches and identical summed
checksums across all four seed tables.

### 2.2 Cross-engine timestamp mismatch (naive timestamps)

**Symptom:** with 2.1 fixed, `cross_engine_identical_checksums_match` failed —
PG and DuckDB produced different checksums for the same data.

**Root cause:** the seed `created_at` columns are **naive** (`timestamp without
time zone` / `TIMESTAMP`), but both dialects applied `AT TIME ZONE 'UTC'`, which
is meant for tz-aware columns. PG's session tz was UTC so it kept the wall-clock;
DuckDB's `CAST(timestamptz AS TIMESTAMP)` re-localized to the machine tz (UTC+2),
shifting the value by two hours.

**Fix (decision — see §4):** render the naive wall-clock directly, with no tz
conversion (`TO_CHAR(created_at, …)` on PG, `CAST(created_at AS VARCHAR)` on
DuckDB). Verified to produce identical per-column checksums across engines.
UTC-normalizing genuinely tz-aware columns is deferred to a future `--assume-tz`
branch.

---

## 3. Restructure this session

- **`vrtb-core` gained `conformance.rs`** — `ChecksumResult`, `Verdict`,
  `whole_table_checksum`, `conformance_check`. It speaks only the `Engine` /
  `Dialect` traits, so it works same-engine and cross-engine and carries no
  driver deps. Unit-tested with an in-memory mock engine (no database).
- **Both engines now implement `vrtb_core::engine::Engine`** (`introspect`,
  `dialect`, `execute`). Previously they had ad-hoc `…EngineExt` traits that
  duplicated the real trait; those are gone.
  - `DuckDBEngine`: sync; `execute` stringifies via `duckdb::types::Value`;
    added `open_read_only` for comparisons.
  - `PostgresEngine`: tokio-postgres is async, the trait is sync — each engine
    owns a **current-thread Tokio runtime** and `block_on`s queries, with the
    connection task spawned onto that runtime. Callers must not already be inside
    a Tokio runtime (so the live tests are plain `#[test]`).
- **CLI implemented** (`run`/`emit` were stubs): `check` and `conformance` build
  both engines from `--src` / `--dst` target strings, introspect, `build_plan`,
  run `conformance_check`, and emit per `--format`. Exit code `0` = match,
  `1` = differ. Target syntax: `postgres://…#schema.table` and
  `duckdb:/path/file.duckdb#table`.
- **`vrtb-integ` removed**; its six cases were re-homed to
  `crates/vrtb-cli/tests/conformance.rs`, now exercising the full stack
  (real engines → introspection → `build_plan` → core `conformance_check`)
  instead of raw SQL.
- **Dead deps/imports removed**: `async-trait` (core, duck), `dotenvy` (pg),
  `tokio` (cli).

### Running the conformance suite

Prerequisites: `docker compose up -d` (PostgreSQL), `python testdata/seed.py`,
and the seeded `data/duckdb/veritable.duckdb`.

```
cargo test -p vrtb-cli --test conformance -- --test-threads=1
```

Or via the binary:

```
veritable conformance \
  --src 'postgres://postgres:PW@localhost:5432/veritable#customers_identical_src' \
  --dst 'duckdb:data/duckdb/veritable.duckdb#customers_identical_src' \
  --key id --columns name --columns email --columns created_at \
  --columns balance --columns active
```

---

## 4. Decisions made

| Decision | Choice | Why |
|---|---|---|
| Where conformance logic lives | In `vrtb-core`, over the `Engine` trait; live DB tests in `vrtb-cli` | `vrtb-core` can't depend on the adapter crates (circular). Keeps core driver-free; the CLI is the only crate that can build both engines. |
| How much of `Engine` to implement | **Full** (`introspect` + `execute` + `dialect`) on both adapters | Lets the CLI introspect → `build_plan` → checksum end-to-end through one trait. |
| Naive timestamp canonicalization | Render the **wall-clock as-is**, no tz conversion | Deterministic and engine-independent for naive columns; the previous `AT TIME ZONE 'UTC'` shifted DuckDB values. tz-aware normalization deferred. |
| PG async → sync | Each engine owns a current-thread runtime and `block_on`s | Keeps the core `Engine` trait synchronous and driver-agnostic. |
| `execute` return shape | `Vec<Vec<String>>`, positional, stringified | Simple, engine-neutral; the checksum row is `(cnt, sum_h1, sum_h2)`. |

---

## 5. What remains (stubs & known limits)

**Stubbed (`todo!()` / `unimplemented!()`):**
- `vrtb-core`: `joindiff`, `hashdiff`.
- Both dialects: `joindiff_sql`, `normalize_column`, `digest_expr`,
  `keyspace_bounds_sql`, `segment_checksum_sql`, `segment_rows_sql`.
- CLI `diff` subcommand (depends on joindiff/hashdiff).

**Known limitations / follow-ups:**
- **`segment_checksum_sql`** will need the same `UBIGINT` formulation as the
  whole-table checksum; the shared SUM/mod block in each dialect is flagged for
  extraction into a private helper so the two stay bit-identical.
- **`build_plan(None)` compares the key only.** Passing no `--columns` checksums
  just the key column. A "compare all shared columns by default" mode is a
  natural follow-up.
- **Generic `execute` type coverage**: PG stringifies bool/int2/4/8/float4/8/
  text/varchar; DuckDB covers all scalars. `NUMERIC` (PG) and temporal/nested
  values fall back to error/Debug. Not hit by the checksum path (which returns
  int8 + text), but needed before row-level diff output.
- **Timestamp precision** is assumed to be 6 (microseconds) on introspection;
  `timestamp(p)` is not yet parsed.
- **tz-aware timestamps** (`--assume-tz`) — UTC normalization for genuinely
  tz-aware columns is not implemented; only naive wall-clock rendering is.
- **Decimal scale negotiation** — the dialects hardcode `NUMERIC(38, scale)`
  from `col.ty`; cross-side scale negotiation (`--coerce-scale`) is not built.

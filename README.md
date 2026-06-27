Veritable - local, cross-database result-set comparison engine in Rust
=========================================================

Veritable is a single-binary engine that proves whether two tables hold the same data, within one database or across different ones.

Status: drafting v0.1
Base language: Rust
Author: Mouad Jaouhari

---
## 0. Personal choices made before building
1. **Name** `veritable`: genuine, true (verity) and it contains "table" (ref to data)
2. **Warehouse priority:** DuckDB + Postgres first (local, free, fast to test against), then cloud datawarehouses will be supported in future versions (Snwoflake, Bigquery, Databricks, ..)
3. **Local-first OSS.** No SaaS, no telemetry, no data leaves the user's machine/warehouse. This is a core selling point for data teams, not a feature to add later (UNNEGOTIABLE).


## 1. Why I am building `veritable` ?
I am building `veritable` to solve real problems (I have faced personally as a DATA ENGINEER). Every replication pipeline, migration, CDC stream, and warehouse switch ends with the same unanswered question: *is the data actually the same on both sides?* The incumbent open-source answer, data-diff, was archived by its vendor; its community successor (reladiff) is a solo-maintained Python fork in beta. No single-binary, compiled, community-owned tool occupies the space. `veritable` is that tool: one binary, two table URIs in, a verdict and a precise difference set out, fast enough for billion-row tables because the work happens inside the databases.

Design identity, stated once and enforced everywhere: **deterministic proof, no daemon, no UI, no cloud, no AI.** It is a verb, like `diff` and `grep`.

## 2. v0.1 scope

Two comparison modes, tiered by cost:
1. **Joindiff** (same connection): both tables reachable from one database — compares via a full outer join on the key. Engine-local, so each new engine costs one SQL dialect.
2. **Hashdiff** (cross-database): tables in different engines — segmented server-side checksums with recursive narrowing. Cross-engine, so each new *pair* costs entries in the type-normalization matrix; this is where the hard correctness work lives (first v0.2 milestone)

Out of scope for all of v0.x: schema diffing/migration generation, data sync, profiling/quality rules, scheduling. One deliberate exception on the roadmap: `--emit-patch` (v0.3), generating the SQL that would repair the target.

## 3. Algorithms

### Joindiff
Fast-exit precheck first: `COUNT(*)` and a whole-table aggregate checksum on both sides; if both match, report identical and stop (the common case must be the cheap case). otherwise, `FULL OUTER JOIN` on the key over normalized column expressions, emitting rows present only left (`-`), only right (`+`), or differing (`~`) with differing columns identified. Results stream and an optional `--materialize <table>` writes them server-side instead.

### Hashdiff
*Coming in v0.2* must figure it out in the meantime

## 4. Connector strategy
The computation is pushed into the databases; the wire carries only checksums and leaf rows.
Transport per engine: native Rust drivers where first-class (`duckdb`, `tokio-postgres`), and in the future, we will be probably using **ADBC driver manager**, I know that this thing exists at least for snowflake  [adbc_snowflake](https://crates.io/crates/adbc_snowflake)

## 5. Project structure

Workspace layout — each concern lives in its own crate:

```
crates/
├── vrtb-core/    # Engine trait, error types, normalization, algorithms (no IO)
├── vrtb-pg/      # PostgreSQL adapter (tokio-postgres)
├── vrtb-duck/    # DuckDB adapter (embedded, no Docker needed)
└── vrtb-cli/     # the "veritable" binary — CLI parsing, output formats, wiring
```

Why split: connectors drag heavy deps (`libduckdb-sys` alone is massive), so each engine is isolated. `vrtb-core` stays pure logic, testable without any database running. New engines = new crate + implement the trait + pass conformance suite.

## 6. Local dev setup

```bash
docker compose up -d          # starts postgres (port 5432)
cargo build                   # compiles everything
cargo run -p vrtb-cli         # runs the veritable binary
```

DuckDB is embedded (via the `duckdb` crate), no container needed — the file lives at `data/duckdb/veritable.duckdb`.

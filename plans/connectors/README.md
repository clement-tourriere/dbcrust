# Wave 3 connectors: research summary

Date: 2026-10-15. Branch `ctourriere/secure-agent-connectors` (HEAD `ab34bb7` + the working-tree lock). Companion to `plans/agent-secure-connections-crates-connectors.md` (sections A and G; owner decisions D3/D4, broker G.3/G.4).

One file per connector: [redis](redis.md), [mssql](mssql.md), [redshift](redshift.md), [libsql](libsql.md), [influxdb](influxdb.md), [s3](s3.md), [dynamodb](dynamodb.md), [cloudwatch](cloudwatch.md), [duckdb](duckdb.md); shared AWS foundation in [aws-common.md](aws-common.md).

## Method (and its limits)

- **Lock baseline**: a copy of the root `Cargo.lock` (1,174 package entries, 1,018 names; includes `aws-lc-rs 1.18.1`/`aws-lc-sys 0.45.0`, still present because `rustls 0.23.45` has its default `aws_lc_rs` feature unified in by `reqwest 0.13`/genai and `quinn-proto`). Each probe project copies that lock, so cargo keeps every existing version and only adds what the candidate needs. "New crates" = `(name, version)` pairs in `cargo tree -e normal` not present in the lock (new names + new *versions* of existing names). Because aws-lc is already in the baseline lock, every probe was also checked with `cargo tree -i aws-lc-rs`: "ring only" below means the probe itself does not pull it.
- **Probes** are tiny binaries (tokio + the candidate, constructing a client; not run against servers except the DuckDB hardening matrix) built with DBCrust's own release profile (`opt-level="z"`, thin LTO, `codegen-units=1`, `strip`, `panic=abort`). Size delta is against a tokio-only probe (389,520 B); "MB" = MiB. Probe dependency lines are in [probes.md](probes.md).
- **Timing**: `~/.cargo/bin/cargo` with a fresh `CARGO_TARGET_DIR` per probe (bypasses the mbx cache on purpose, so these are cold numbers; sources were already in `~/.cargo/registry`, download time excluded). Machine: 12-core Apple silicon shared with another agent's builds (load average 5-25), so treat wall times as +/-30 %; **CPU = user+sys seconds** is the more stable figure. `mise exec` was not used (builds happen outside the repo; no tracked file, `Cargo.toml` or `Cargo.lock` of the repo was touched; scratch dir deleted at the end).
- **Cooldown**: every new pair was checked against the crates.io API (`created_at`, `yanked`). Only one violation found: `combine 4.6.9` (published 2026-10-15 13:17 UTC) when resolving `redis`; pin `combine 4.6.8` (2026-08-25).
- **Audit**: `cargo audit` (advisory DB of 2026-10-14) on each probe's pruned lock. The only advisories common to *all* probes and to DBCrust's own lock are `rustls 0.23.45` (RUSTSEC-2026-0346/-0347/-0348/-0349, fixed by 0.23.46 published 2026-10-14 09:54 UTC, now older than 24 h) and `quinn-proto 0.11.17` (0338/0340/0343, fixed 0.11.18). Bump those independently of any connector.

## Summary table

"New crates" are pairs not in the lock. Times: wall / CPU seconds. "Size" = stripped probe minus 389 KB baseline.

| Connector | Crate @ version (age) | New crates | Clean release build | Size delta | Native read-only? | TLS / ring OK? | Default or opt-in | Effort |
|---|---|---|---|---|---|---|---|---|
| Redis | `redis` =1.7.1 (20 d) | **5** (6 with `cluster-async`) | 16 s wall / 51 s CPU | +1.26 MB | **Conditional**: native if `ACL DRYRUN` proves write/admin denied (Redis >= 7); else client heuristic | Yes (rustls nodef; must `install_default(ring)`) | default | M |
| SQL Server | `tiberius` =0.13.0 (20 d) **+ 4-line patch** | **7** | 14 s wall / 53 s CPU (upstream, aws-lc: 27 s wall / 102 s CPU) | +1.24 MB (upstream: +2.01 MB) | **No** (server role/db only; probe-verified, `ApplicationIntent` is routing not enforcement) | **Only with patch**: 0.13.0 hard-codes aws-lc-rs; 0.12.3 is ring but 3 RUSTSEC | default once patched, else opt-in | M-L |
| Redshift | existing `sqlx` 0.8.6 PG | **0** | none | ~0 | Transaction-level `BEGIN READ ONLY` (documented; verify in spike) + read-only user | Yes (existing) | default | S-M |
| libSQL / Turso (remote) | hand-rolled Hrana on `reqwest` 0.12.28; (`libsql` 0.10.0 / `turso_serverless` 0.1.3 evaluated) | **0** (libsql: 20; turso_serverless: 1) | code only (libsql 20 s wall / 74 s CPU; turso 30 s wall / 133 s CPU) | ~0.05 MB est. (libsql +2.02 MB; turso +2.59 MB) | Token scope (JWT `a:"ro"`) + Hrana `describe.is_readonly`; not client-provable otherwise | Yes (reqwest 0.12 ring). `libsql`: ring but **5 RUSTSEC** (h2 0.3, webpki 0.102); `turso_serverless`: forces reqwest 0.13 default-tls = aws-lc | default | M |
| InfluxDB v1/v2/(v3) | hand-rolled `reqwest` 0.12 + `csv` (`influxdb` 0.8.0 evaluated) | **0** (`influxdb` crate: 3) | code only | ~0.1 MB est. (`influxdb` crate: +1.56 MB) | v1 over GET: server-restricted to SELECT/SHOW (native); v2/v3: token scope, unprovable -> heuristic | Yes (reqwest 0.12). `influxdb` crate default = reqwest 0.13 aws-lc | default | M |
| S3 / R2 / MinIO | `aws-sdk-s3` =1.152.0 (14 d) on shared AWS base | **34** (29 base + 5) | 46 s wall / 159 s CPU | +3.74 MB | **No** (IAM); typed read-only surface = `ClientStructural` | Yes with `aws-smithy-http-client` `rustls-ring`, `default-features=false`, injected client. SDK defaults = aws-lc (54 crates). `object_store` aws: quick-xml RUSTSEC / aws-lc | opt-in | M (+M for DataFusion adapter) |
| DynamoDB | `aws-sdk-dynamodb` =1.130.0 (14 d) | **+2** (31 standalone) | 34 s wall / 136 s CPU | +3.66 MB | **No** (IAM; `FullTableScan` condition key helps) | Yes (same as S3) | opt-in | M |
| CloudWatch Logs | `aws-sdk-cloudwatchlogs` =1.156.0 (14 d) | **+3** (32 standalone) | 48 s wall / 156 s CPU | +3.60 MB | **No** (IAM); read API only (`ClientStructural`); `StopQuery` = native cancel | Yes (same) | opt-in | M |
| AWS base (shared) | `aws-config` =1.12.0 + `aws-smithy-http-client` =1.5.0 | **29** | 23 s wall / 120 s CPU | +3.44 MB (3 services: +4.00 MB; no-sso/no-process base: +3.32 MB) | n/a | Yes (config above); **MSRV 1.94.1** (> DBCrust 1.91) | opt-in (`aws`) | M |
| DuckDB (+DuckLake) | `duckdb` =1.10506.0 / engine 1.5.6 (15 d) | **7** | 159 s wall / 1056 s CPU bundled (`+parquet,json`: 5 m 02 s wall / 1359 s CPU; non-bundled download mode: 30 s / 125 s) | +19.11 MB bundled (+parquet/json 22.5 MB total; non-bundled 0.77 MB + 117 MB shared lib) | **Yes** for file DBs: `access_mode=READ_ONLY` + `enable_external_access=false` + `lock_configuration` (verified by probe) | No TLS in the crate (extensions fetched over HTTPS by DuckDB itself, disabled for agents); C++ build | opt-in | L (+M DuckLake) |

## Key risks and findings

1. **aws-lc-rs/OpenSSL**
   - Forcing aws-lc: `tiberius 0.13.0` (hard-coded provider + `tokio-rustls` default features; needs a 4-line patch), `turso_serverless` and `influxdb` (reqwest 0.13 `default-tls`), the AWS SDK *defaults* (`default-https-client` -> `rustls-aws-lc`, `prefer-post-quantum`), `object_store 0.14` `aws`.
   - All of them have a ring path except `turso_serverless` (hand-roll instead) and `object_store`.
   - OpenSSL via native-tls: only the *defaults* of `tiberius` and `influxdb2` (never enabled), plus dbflux's `postgres-native-tls` (not reused).
   - Because `redis`/`tiberius`/reqwest 0.13 call `rustls::ClientConfig::builder()`, **DBCrust must install the ring `CryptoProvider` once at startup** (`install_default`); with both providers compiled `builder()` is ambiguous and panics. Put it in the new `tls` module of `dbcrust-core`.
2. **Large C/C++ builds**: only DuckDB `bundled` (2.7 min wall on a quiet 12-core machine, 4-5 min under load; ~18 CPU-minutes cold; +20 MB). Everything else is pure Rust. The AWS stack is ~100-200 CPU-seconds, dominated by generated service crates.
3. **24 h cooldown**: no violation in the final selections once `combine` is pinned to 4.6.8. `libsql 0.10.0` was ~44 h old (OK but moot: rejected). The AWS SDK republishes every few days - always pin with `=` and re-run `check_cargo_cooldown.py`. `tiberius` patched via git fork would need the cooldown script taught about git sources.
4. **Advisories**: `libsql 0.10.0` (5), `tiberius 0.12.3` (3), `object_store 0.13.2` (quick-xml, 2) - all rejected on policy. Everything selected is clean beyond the shared `rustls 0.23.45`/`quinn-proto 0.11.17` baseline.
5. **MSRV**: the AWS crates require Rust 1.94.1; DBCrust declares 1.91. DuckDB needs 1.85.1 (fine).
6. **Read-only is rarely native**: of nine connectors only DuckDB (file) is truly engine-enforced out of the box; Redis can be *proven* via `ACL DRYRUN`; libSQL via token claim; InfluxDB v1-GET via transport; Redshift via `BEGIN READ ONLY` (to verify). SQL Server, S3, DynamoDB and CloudWatch depend on IAM/roles the client cannot prove -> the broker must show them as not natively read-only and keep "structural/heuristic" labelling. This fits G.3/G.4's rule that each connector declares its enforcement before enrollment.
7. **32-bit wheels** (CLAUDE.md): DuckDB and AWS stacks must be excluded from i686 builds; the default-on connectors are pure Rust and fine.
8. **No cancellation** for Redis, SQL Server (no TDS attention), Turso; native for PG/Redshift (`CANCEL`), DuckDB (`interrupt`), CloudWatch (`StopQuery`), S3/DynamoDB (drop future).

## Recommended build order inside Wave 3 (matches D4)

1. `tls` helper + `SecretMap` + `DriverDescriptor`/capability API (below) - one PR, no connector.
2. Redis (smallest, proves the non-SQL path), SQL Server (needs the tiberius patch decision first).
3. Redshift (spike first), libSQL (hand-rolled Hrana).
4. `dbcrust-aws` base, then S3, DynamoDB, CloudWatch; InfluxDB in parallel (no AWS dependency).
5. DuckDB bundled (separate CI job), then DuckLake.

## Recommended common driver API (for the `dbcrust-core` split)

Constraints: additive to the existing `DatabaseClient` (`src/database.rs:1219`) and `MetadataProvider` (`:1141`); every new trait method has a default so the seven existing drivers compile unchanged; `StructuredQueryResult` (rows of `Option<String>`) stays the universal result; `DatabaseError::FeatureNotSupported` stays the "unsupported" error (no silent fallback).

### 1. Capability flags (answers G.3/G.4 "native vs heuristic")

```rust
/// Strength of a guarantee. The broker's `native_read_only: bool` is `ServerEnforced` only.
pub enum Enforcement {
    ServerEnforced,   // engine/IAM/token rejects it (PG default_transaction_read_only, DuckDB access_mode, ACL)
    ClientStructural, // the client API surface has no operation that can do it (S3/CloudWatch typed ops)
    ClientHeuristic,  // textual/statement classification (safety.rs)
    Unsupported,
}
pub enum EngineFamily { Relational, KeyValue, Document, Search, TimeSeries, LogStream, ObjectStore, WideColumn }
pub enum QueryLanguage { Sql(SqlDialect), RedisCommand, PartiQl, LogsInsights, InfluxQl, Flux, Qualifier, Native }
pub enum LimitMode { ServerSide, FetchSide, PostFilter, Unsupported }   // FetchSide = stop reading + drop; PostFilter = whole reply read, then cut
pub enum CostUnit { Bytes, Rows, Rcu, GbScanned, Requests }

pub struct DriverCapabilities {
    pub family: EngineFamily,
    pub languages: &'static [QueryLanguage],
    pub read_only: Enforcement,          // baseline; refined per connection by `verify_read_only`
    pub statement_timeout: Enforcement,  // ServerEnforced = PG statement_timeout; ClientHeuristic = tokio timeout + drop
    pub cancellation: Enforcement,       // pg cancel, StopQuery, DuckDB interrupt = ServerEnforced; drop-connection = ClientHeuristic
    pub row_limit: LimitMode,
    pub byte_limit: LimitMode,
    pub cost_unit: Option<CostUnit>,     // what `QueryStats.consumed` reports (RCUs, GB scanned...)
    pub parameter_binding: bool,
    pub transactions: bool,
    pub multi_database: bool,
    pub hierarchy_depth: u8,             // 2 = schema.table, 3+ = bucket/prefix/key
    pub typed_ops: &'static [OpKind],    // Browse, Describe, Get, Sample, Tail
}
```

Declared values (feeds `ConnectionSummary.native_read_only`, which should grow a string `read_only_kind` with `#[serde(default)]` for protocol back-compat):

| Engine | read_only | statement_timeout | cancellation | row_limit | byte_limit | cost_unit |
|---|---|---|---|---|---|---|
| PostgreSQL/MySQL/SQLite/ClickHouse (today) | ServerEnforced (connect hardening) | ServerEnforced | ServerEnforced/Heuristic | FetchSide | FetchSide | Rows |
| Redis | ServerEnforced if ACL-proven else ClientHeuristic | ClientHeuristic | ClientHeuristic (drop conn) | PostFilter (reply fully read; bound the request) | PostFilter | Requests |
| SQL Server | ClientHeuristic (ServerEnforced only if role probe passes) | ClientHeuristic | ClientHeuristic (drop conn) | FetchSide (+`SET ROWCOUNT`) | FetchSide | Rows |
| Redshift | ServerEnforced (txn `READ ONLY`, to verify) | ServerEnforced | ServerEnforced | FetchSide | FetchSide | Rows |
| libSQL | ServerEnforced if token `ro` else ClientHeuristic | ClientHeuristic | Unsupported | FetchSide (v3 cursor) / PostFilter (v2) | PostFilter | Rows |
| InfluxDB | ServerEnforced (v1 GET) else ClientHeuristic | ClientHeuristic | ClientHeuristic | FetchSide | FetchSide | Bytes |
| S3 | ClientStructural | ClientHeuristic (SDK timeouts) | ServerEnforced (abort request) | ServerSide (`max_keys`) | ServerSide (`Range`) | Requests/Bytes |
| DynamoDB | ClientStructural (IAM unproven) | ClientHeuristic | ClientHeuristic | PostFilter (page read, then cut) | PostFilter | Rcu |
| CloudWatch Logs | ClientStructural | ClientHeuristic | ServerEnforced (`StopQuery`) | ServerSide (`limit`) | ServerSide (window/bytes cap) | GbScanned |
| DuckDB | ServerEnforced (file) | ClientHeuristic (timer -> interrupt) | ServerEnforced (`interrupt`) | FetchSide | FetchSide | Rows |

### 2. New `DatabaseClient` methods (all defaulted)

```rust
pub trait DatabaseClient {
    // existing methods unchanged ...

    /// Static baseline. Default: relational SQL, heuristic read-only, FetchSide limits.
    fn capabilities(&self) -> DriverCapabilities { DriverCapabilities::relational_default() }

    /// Per-connection proof used at broker enrollment (Redis ACL DRYRUN, MSSQL HAS_PERMS_BY_NAME,
    /// DuckDB current_setting('access_mode'), JWT claim...). Never writes. Default: echoes capabilities().
    async fn verify_read_only(&self) -> Result<ReadOnlyReport, DatabaseError> { /* default */ }

    /// Limits-aware execution = the broker gateway's single execution path (G.4 step 5).
    /// Default wraps `execute_query_structured` with tokio timeout + row/byte truncation.
    async fn execute_limited(&self, input: &str, limits: &ExecutionLimits, cancel: &CancelToken)
        -> Result<LimitedResult, DatabaseError> { /* default */ }

    /// Typed hierarchical browse for engines deeper than schema.table (S3 prefixes, Redis key prefixes,
    /// log groups). Default delegates to MetadataProvider::get_schemas/get_tables/get_columns.
    async fn browse(&self, req: BrowseRequest) -> Result<BrowseResult, DatabaseError> { /* default */ }
}

pub struct ExecutionLimits {
    pub max_rows: Option<u32>, pub max_bytes: Option<u64>, pub timeout: Option<Duration>,
    pub max_cost: Option<f64>,          // in capabilities().cost_unit (bytes scanned, RCUs...)
    pub time_window: Option<TimeWindow>,// mandatory for LogStream/TimeSeries engines
    pub page: Option<PageToken>,
}
pub struct LimitedResult {
    pub result: StructuredQueryResult,  // unchanged universal rows
    pub truncated: Truncation,          // None | Rows | Bytes | Cost | Page(PageToken)
    pub stats: QueryStats,              // rows_read, bytes_read, consumed: Vec<(CostUnit, f64)>, duration
    pub notices: Vec<String>,           // multi-result-set notes, consumed RCUs, "full scan" warnings
}
pub enum BrowseRequest {
    Containers { parent: Vec<String>, page: Option<PageToken> },   // buckets, schemas, DBs, log groups
    Entries    { container: Vec<String>, filter: Option<String>, page: Option<PageToken> },
    Describe   { path: Vec<String> },                              // -> TableDetails-compatible
    Sample     { path: Vec<String>, limit: u32 },
}
```

`StructuredQueryResult` is **not** modified (struct literals exist across crates); richer metadata lives in `LimitedResult`. Non-tabular replies are rendered as JSON text in cells with a `shape` hint in `notices` or a future `ResultShape` field.

### 3. Pure statement classifier per driver (no connection needed)

```rust
pub trait StatementClassifier: Send + Sync {
    fn classify(&self, input: &str) -> Classification; // { class: Read|Write|Ddl|Admin|Unknown, single: bool, reasons, bounded_form: Option<String> }
}
```

`safety::check_read_only(query, &DatabaseType)` (currently `match` with `check_mongodb`/`check_elasticsearch`/`check_white_dragon`/`check_sql`, `src/safety.rs:30`) becomes a shim over `driver_registry().classifier(db_type)`; the broker classifies *before* connecting and unknown means refuse (G.4 step 4). Per-engine classifiers described in each connector file (Redis `COMMAND INFO` flags + allow list, T-SQL tokenizer, InfluxQL/Flux allow-list, PartiQL SELECT-only, DuckDB `json_serialize_sql`, Hrana `describe`).

### 4. One descriptor instead of eleven `match` arms

Adding White Dragon touched 17 files with exhaustive `DatabaseType` matches (`database.rs` has 11 `DatabaseTypeExt` methods, plus `url_scheme.rs`, `config.rs::legacy_database_type`, `cli_core.rs` x3, `completion.rs`, `sql_parser_trait.rs`, `safety.rs`, gui/core `url_builder.rs`/`schema.rs`/`connect.rs`/`types.rs`...). Replace with:

```rust
pub struct DriverDescriptor {
    pub id: &'static str,                      // "redis"
    pub display_name: &'static str,
    pub schemes: &'static [&'static str],
    pub default_port: Option<u16>,
    pub file_based: bool, pub ssh_tunnel: bool, pub requires_auth: bool, pub json_explain: bool,
    pub sql_functions: &'static [&'static str],
    pub secret_fields: &'static [SecretField], // which option keys / SecretMap names are secret (feeds redaction + store)
    pub binding: fn(&ConnectionInfo) -> TargetBinding, // engine/host/port/database/user/tls (O3)
    pub capabilities: fn(&ConnectionInfo) -> DriverCapabilities,
    pub classifier: &'static dyn StatementClassifier,
    pub connect: fn(ConnectionInfo, ConnectOptions) -> BoxFuture<'static, Result<Box<dyn DatabaseClient>, DatabaseError>>,
    pub cargo_feature: Option<&'static str>,   // None = always compiled
}
```

- `DatabaseType` stays a flat enum; **add all new variants unconditionally** (`Redis`, `SqlServer`, `Redshift`, `LibSql`, `InfluxDb`, `S3`, `DynamoDb`, `CloudWatch`) so serde, strum iteration and exhaustive matches stay stable across feature sets. When a driver's feature is off, `create_database_client` returns the existing `DatabaseError::FeatureNotSupported { feature: "<engine> support is not compiled in (feature `redis`)" }`.
- The registry is a `fn registry() -> &'static [&'static DriverDescriptor]` assembled with `#[cfg(feature = ...)]` in the root (or `inventory`-style per crate after the split); the GUI's `url_builder` forms derive from `schemes`/`default_port`/`secret_fields` instead of hand-written arms.

### 5. Connect options and secrets as data, not globals

Today `read_only_requested()`, `query_timeout()`, `interrupt_flag()` and `ONE_SHOT_MODE` are process-wide atomics (`src/database.rs:23-57,70`). Connectors should receive:

```rust
pub struct ConnectOptions {
    pub read_only: bool,                 // connect-time hardening; the driver reports via capabilities()
    pub statement_timeout: Option<Duration>,
    pub interrupt: Arc<AtomicBool>,      // per-connection, replaces the thread-local/global flag
    pub application_name: String,
    pub tls: TlsPolicy,                  // verify-full | verify-ca | require | insecure (insecure refused for broker aliases)
}
```

and `ConnectionInfo` gains `secrets: SecretMap` (`BTreeMap<&'static str, Zeroizing<String>>`, redacting `Debug`; canonical names `password`, `token`, `secret_access_key`, `session_token`, `key_passphrase`, `sentinel_password`). `password` stays for compatibility and is the `password` entry. This is required by AWS (three secrets), Turso/InfluxDB (tokens) and mTLS keys, and keeps `bound_target` + redaction (G.2/O3, W1-T1) uniform.

### 6. Conformance harness (`dbcrust-core::testkit`)

An rstest-driven suite every driver runs: URL round-trip + redaction canary (the W1-T1 canary matrix), classifier corpus per language, `ExecutionLimits` truncation semantics against a `FakeDriver`, `verify_read_only` never writes (fake records calls), `capabilities()` consistent with behaviour. Live endpoints are env-gated exactly like `DATABASE_URL`: `REDIS_URL`, `MSSQL_URL`, `REDSHIFT_URL`, `LIBSQL_URL`+`LIBSQL_TOKEN`, `INFLUX_URL`+`INFLUX_TOKEN`, `AWS_ENDPOINT_URL`+`S3_BUCKET`/`DYNAMODB_TABLE`/`CWL_GROUP`, `DUCKLAKE_TEST`; DuckDB needs no gate (bundled, temp files).

### 7. Prerequisites to land before the first connector

1. Single `tls::ensure_ring_provider()` and removal of aws-lc from the tree (Wave 2a) - or an explicit decision to tolerate it.
2. Raise the secret-ish redaction key list (`authtoken`, `secret_access_key`, `session_token`, `sentinel_password`, `influx_token`, `data_path_secret`) in `password_sanitizer` and the `TargetBinding` mapping per engine.
3. MSRV decision (1.94.1) before the `aws` feature; keep `aws`/`duckdb` out of the 1.91 and i686 CI jobs.
4. Decide the tiberius fork/patch vs upstream PR; teach `check_cargo_cooldown.py` about git-sourced entries if forked.

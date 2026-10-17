# CLAUDE.md

DBCrust — high-performance multi-engine interactive client in Rust with Python bindings. Supports PostgreSQL, MySQL, SQLite, MongoDB, ClickHouse, Elasticsearch, White Dragon, Redis/Valkey, SQL Server/Azure SQL, Amazon Redshift, libSQL/Turso, InfluxDB (v1 InfluxQL, v2 Flux, v3 SQL), Amazon S3/R2/MinIO, DynamoDB and CloudWatch Logs (opt-in AWS connectors), DuckDB/DuckLake (opt-in `duckdb` feature, embedded C++ engine), and file formats (Parquet, CSV, JSON) via Apache DataFusion.

## Build Commands

`mise.toml` requires mise 2026.8.16+ and routes Cargo through the pinned Mr Boxington build cache. Run `mise install` once; use `mise run` tasks (or an activated mise shell) so Cargo is wrapped. Diagnose cache setup with `mbx doctor`.

```bash
mise install
mise run build:dev        # dev build
mise run build            # release build
mise run build:slim       # release build of the slim `standard` preset (PostgreSQL, MySQL, SQLite)
mise run check:features   # cargo check of zero-engine, standard and each feature alone
mise run install          # install dbcrust + dbc into ~/.cargo/bin
mise run test             # run tests
cargo test -- --nocapture # tests with output
cargo run -- postgres://localhost/test  # run CLI directly

# Python bindings
mise run py:dev           # maturin dev build
mise run py:build         # wheel

# Desktop app (GPUI Kit, pure Rust) — see gui/ARCHITECTURE.md
mise run gui:dev          # run it
mise run gui:test         # backend tests (dbcrust-gui-core, no GPUI)
mise run gui:lint         # clippy on both GUI crates (--no-deps)
mise run gui:screenshots  # regenerate docs/README screenshots headlessly

# Build cache — Mr Boxington (mbx), the `[wrappers.cargo]` entry in mise.toml
mise run cache:stats      # savings + disk use
mise run cache:gc         # reclaim disk space (`-- --dry-run` to preview)
```

- Run Cargo through mise (`mise run …` or `mise exec -- cargo …`) so builds hit the shared mbx cache; calling `~/.cargo/bin/cargo` directly bypasses it. mbx turns `target/` into a link to a managed directory with a disk budget.
- No mise installed? `./bin/mise` (committed wrapper, pinned mise version) works for every command above; regenerate it with `mise generate install-script --write --windows`.

## Module Map

| File | Purpose |
|------|---------|
| `crates/core/` | `dbcrust-core` (lib `dbcrust_core`, layer 0: no driver SDK, HTTP, Docker or TUI deps): `database` (client traits, `ConnectionInfo`, `DatabaseError`), `model`, `options`, `docker_target`, `output_format`, `password_sanitizer`, `safety`, `format`, `complex_display`/`vector_display`/`geojson_display`/`json_display`, `performance_analyzer`, `regex_operators`, `sql_parser`, `sql_buffer` (lexer/splitter; the reedline `SqlValidator` stays in root), `redis_command` (`redis-cli` tokenizer, script splitter, `COMMON_COMMANDS`; shared by the Redis driver, the read-only guard and completion), `s3_command` (the S3 command language: `Verb` enum via strum, tokenizer, script splitter, `parse` → `S3Command` — no variant writes, so the guard accepts exactly what parses; shared by the S3 driver, the guard, completion and the CLI's statement splitting), `dynamodb_command` (the DynamoDB input language: five verbs via strum + one PartiQL `SELECT`, with a lexer that sees through comments/quoted identifiers/literals — `parse` is the single judge, `safety::check_dynamodb` accepts exactly what parses and the driver runs exactly that; no write exists), `cloudwatch_command` (the CloudWatch Logs language: `groups`/`streams`/`tail`/`use`/`help` + Logs Insights queries with a `since`/`between`/`--group` prefix, durations/timestamps; the first word of a query must be an Insights command; same single-judge rule), `explain_plan`, `ai_config`, `tls` (feature `tls`). Cargo features `usage` (derive `ValueEnum` on `OutputFormat`) and `tls`; root enables both and re-exports every module under its old path (`crate::database::…`, `dbcrust::format::…`); `database::create_database_client` is added by root's `database` facade because the factory lives in `src/drivers.rs`. `regex`/`serde_json` feature sets are declared explicitly so core works standalone (`cargo test -p dbcrust-core`) |
| `src/main.rs` | Entry point, Tokio runtime, CLI orchestration |
| `src/lib.rs` | PyO3 bindings (`PyDatabase`, `PyConfig`, `run_cli_loop`); `ai_config_status`/`run_ai_investigation` stay registered as stubs without the `ai` feature |
| `src/features.rs` | `FeatureNotCompiled` (`Docker`/`Ai`/`ExplainTui`/`Clipboard`): the typed "not compiled into this build" error of the add-on features |
| `src/cli_core/ai.rs` | `??`, `???` and the `\ai` wizards (`impl CliCore`, compiled only with feature `ai`) |
| `src/commands.rs` | **Enum-based command system** — all `\cmd` logic lives here |
| `src/cli.rs` | Arg parsing via `usage-rs` derive (jdx/usage) — private `Root` wraps public `Args`; one-shot flags (`-c`/`-f`, `-o/--format`, `--read-only`, `--no-input`); `OutputFormat`/`OneShotSource`/`ParseExit` live here; hidden `__complete_word__`/`__usage_spec__` endpoints answered in `parse_from_argv` |
| `src/shell_completion.rs` | Rust-side URL completer (`complete_connection_url`: schemes, sessions, docker, files) + completion-script generation (usage scripts call the binary back at Tab time) |
| `crates/core/src/safety.rs` | Read-only statement classification — `--read-only` guard + AI agent guard share it; `classify_for_broker` is the broker's lexer-based classifier (default-deny) |
| `src/agent_guide.md` | Embedded `dbcrust agents` guide (the one-shot contract for AI coding agents) |
| `crates/ai/` | `dbcrust-ai` (lib `dbcrust_ai`, layer 2: core [`tls`] + `dbcrust-secrets` + genai/reqwest; no driver SDK, Docker, TUI or root): the AI assistant (`??` text-to-SQL, `\ai`) — multi-provider via `genai` (`reqwest_genai` = its reqwest 0.13 with `rustls-no-provider`; the `genai =0.7.0-rc.4` pin lives in this manifest), ChatGPT sign-in, key storage via `dbcrust-secrets`. Talks to the database only through the `AiSession` trait (`session.rs`); root implements it for `Database` in `src/ai_session_impl.rs` and re-exports the crate as `dbcrust::ai` (`ai::AdapterKind` re-exports genai's provider enum so root has no genai dependency). Config types live in `crates/core/src/ai_config.rs` (`ai::config`). Has its own `test_isolation` canaries (dev-dependency `dbcrust-secrets/test-mode`) and the genai TLS regression tests (`tls_tests.rs`) |
| `src/update.rs` | `--update`: install-channel detection + GitHub release check |
| `src/config.rs` | TOML config, `save_with_documentation` must be updated for new fields |
| `crates/secrets/` | `dbcrust-secrets` (re-exported as `dbcrust::secrets`): `SecretBackend` trait, `KeychainBackend`, Argon2id `PassphraseFileBackend` (`secrets.enc`), `SecretStore` facade, target-bound `SecretRecord`/`AgentPolicy`, non-secret `secrets_index.toml`. `paths` owns the config-dir resolution, `is_test_mode` (feature `test-mode` / `force_test_mode()` / `RUST_TEST_MODE` / thread name) and `write_file_atomically` (`config::` re-exports them); `hints` the one-time stderr notices; `legacy::{dbcrust_pass, password_encryption, vault_encryption, ai_keys}` the read-only legacy readers (root re-exports the first three under their old module names). Features: `prompt` (inquire passphrase prompt — root enables it, the broker need not), `test-mode`/`test-support` (dev-dependency only). No CLI/config/driver/HTTP deps (`cargo tree -p dbcrust-secrets` stays clear of sqlx, datafusion, reqwest, genai…); tests get an in-memory store |
| `src/secrets_cli/` | `dbcrust secrets status\|audit\|migrate\|unlock\|lock\|delete` (no connection needed) |
| `crates/broker/` | `dbcrust-broker` (lib `dbcrust_broker`, layer 3 — the credential-holding process; Unix modules): `paths` (user/service modes, state dir, permission checks), `registry` (`aliases.toml` descriptors + authoritative `SecretRecord`s), `admin` (enroll/revoke/policy), `gateway` (the only execution path: resolve → bind → classify → limit → audit → run), `audit` (`audit.jsonl`, fail closed), `server` (Unix socket + handshake), `hardening` (umask/core/dumpable/env scrub; applied after any privilege drop), `privileges` (**trusted-binary rule**: service-mode `serve` and root admin commands refuse a binary that is not root-owned with root-owned, non-group/world-writable ancestors — `--allow-untrusted-binary` is dev-only; verified privilege drops), `service` (`dbcrust broker install-service`/`uninstall-service`: pure plan/plist/unit generators + dry-run, launchd `_dbcrust` / systemd `dbcrust`, root-owned copy at `/usr/local/libexec/dbcrust-broker`; prompts through the `ServicePrompts` trait), `testkit` (fixtures, behind feature `test-support`). Depends on core, secrets (without `prompt`), drivers, agent-protocol only — `boundary_tests` fails if `inquire`/`reedline`/`usage`/the root crate/AI/Docker show up in its manifest, and `cargo tree -p dbcrust-broker` must stay free of them. Wire contract: `crates/agent-protocol`; user guide: `docs/src/content/docs/user-guide/agent-connections.md` |
| `src/broker/` | Broker **front end** (root crate): `mod.rs` re-exports `dbcrust_broker` (`dbcrust::broker::admin`, …), `cli.rs` (`dbcrust broker …`; with `run` and `e2e_tests` the only code that names `crate::cli`, `inquire`, `OutputFormat`; prompts reach `service` through `TerminalPrompts: ServicePrompts`), `e2e_tests.rs` (drives `cli::run` against the real core; Postgres cases gate on `DATABASE_URL`) |
| `crates/secrets/src/legacy/` | `~/.dbcrust` reader/migration source (`dbcrust_pass`), decrypt-only `enc:` reader (`password_encryption`), Vault credential cache (`vault_encryption`), retired AI-key probes (`ai_keys`) — never write new passwords here |
| `src/config_editor.rs` | Schema-driven `\config` menu/get/set + SSH tunnel manager (`dbcrust config` CLI) |
| `src/prompt.rs` | Reedline interactive REPL |
| `src/completion.rs` | SQL autocomplete with metadata caching |
| `crates/core/src/ddl.rs` | `TableCatalog` + `collect_table_names`/`format_table_ddl`: shared by `\ddl` and the AI schema context, so `\ddl` works without the `ai` feature |
| `crates/core/src/format.rs` | Output formatting (table, expanded, CSV, JSON, JSONL) — `render_query_results` is the dispatch |
| `crates/core/src/database.rs` | `DatabaseClient` + `MetadataProvider` traits |
| `crates/drivers/` | `dbcrust-drivers` (lib `dbcrust_drivers`, layer 1: core + the driver crates only): the `connectors!` table (`"<feature>" => engines => client type`, one row per driver crate), `create_database_client`, `is_compiled`/`compiled_engines`/`required_feature`. One cargo feature per driver forwarding to an optional `dep:dbcrust-driver-*`; default = all drivers (workspace entry is `default-features = false`, the root presets pick). A disabled engine returns `DatabaseError::ConnectorNotBuilt { engine, feature }` |
| `crates/driver-sqlite/` | `dbcrust-driver-sqlite` (`SqliteClient`): may depend on `dbcrust-core` and its SDK (sqlx `sqlite`, no TLS) only |
| `crates/driver-redis/` | `dbcrust-driver-redis` (`RedisClient`): `redis` =1.7.1 (tokio + rustls, no bundled provider: ring is installed through `dbcrust-core`'s `tls` feature). The "query" is one command line; `reply` maps RESP shapes to tables, `metadata` browses with bounded `SCAN` (never `KEYS`), `conn` owns the `ConnectionManager` and the timeout/interrupt loop, `connect` maps `ConnectionInfo` (ACL user, `/N`, `rediss`, unix socket). Read-only classification lives in `crates/core/src/safety.rs` (`check_redis`, default-deny); the broker refuses Redis enrollment (`unsupported_engine`). Fake-server tests need no Redis; real-server tests gate on `REDIS_URL` |
| `crates/driver-mssql/` | `dbcrust-driver-mssql` (`MssqlClient`, `DatabaseType::SqlServer`): TDS through the **vendored** `tiberius` (`third_party/tiberius`, rustls on ring only; see Rust dependency policy). `conn` builds the `Config` from `ConnectionInfo` (`encrypt=true\|false\|strict`, `trustServerCertificate` opt-in, `ca_cert`, named instances via SQL Browser), owns the single pinned session (`Shared`: reconnects after a drop, timeout/Ctrl-C → TDS attention, `max_rows` cap, `SET SHOWPLAN_XML` plans), `value` formats every `ColumnData` like SSMS, `metadata` browses the `sys.*` catalog with parameters only. Read-only classification lives in `crates/core/src/safety.rs` (`check_sql_server`, default-deny T-SQL lexer); the broker refuses SQL Server enrollment (`unsupported_engine`). SQL Server has no session read-only mode. Real-server tests gate on `MSSQL_URL` (`mcr.microsoft.com/mssql/server:2022-latest`, `trustServerCertificate=true`) |
| `crates/driver-redshift/` | `dbcrust-driver-redshift` (`RedshiftClient`, `DatabaseType::Redshift`): the same sqlx `postgres` as `driver-postgres` but its own client on purpose (Redshift is PostgreSQL 8.0.2: no extended-protocol type lookups, no `options=-c` startup, no modern `pg_catalog`, text EXPLAIN only). `conn` builds the connect options (`sslmode` default `require`, `sslrootcert`, `SET statement_timeout` after connect) and owns the pinned simple-query session (Ctrl-C/timeout → `pg_cancel_backend`, `--read-only` → `BEGIN READ ONLY … ROLLBACK`, a transaction-scoped extra rather than a server guarantee), `value` renders every cell from its text form (SUPER = JSON text), `metadata` browses `SVV_*` (+ best-effort `PG_TABLE_DEF`/`PG_CONSTRAINT`) with escaped literals only. Read-only classification: `crates/core/src/safety.rs` (`check_redshift`); the broker refuses Redshift enrollment (`unsupported_engine`: no session-level read-only to enforce). Fake PostgreSQL-protocol server tests need no cluster; the live test gates on `REDSHIFT_URL` |
| `crates/driver-libsql/` | `dbcrust-driver-libsql` (`LibSqlClient`, `DatabaseType::LibSql`): **remote only**, a hand-written Hrana-over-HTTP (JSON, `/v2/pipeline`) client on the locked `reqwest` 0.12 (ring) — no `libsql` crate (5 RUSTSEC, second HTTP/TLS stack) and no `turso_serverless` (forces reqwest 0.13 + aws-lc). `hrana` has the wire types, value decoding and the HTTP/error mapping (token scrubbed everywhere, `base_url` followed only on the same scheme, no redirects); `lib` runs statements stateless (`execute`+`close`) except `BEGIN`…`COMMIT` which carries the baton, and `--read-only` asks `describe.is_readonly` per statement; `metadata` reads `sqlite_schema` + `pragma_*` (two round trips for `\d`). The auth token is the **password slot** (URL `?authToken=` is moved there by `normalize_libsql_connection`; secret store, binding includes the `tls` option). Plain HTTP only for loopback or an explicit `tls=false`. Read-only classification: `crates/core/src/safety.rs` (`check_libsql`, PRAGMA allowlist); the broker refuses libSQL enrollment (`unsupported_engine`: a token's scope cannot be proven). Fake Hrana server tests need no sqld; the live test gates on `LIBSQL_URL` (`docker run -p 8080:8080 ghcr.io/tursodatabase/libsql-server:latest`, `libsql+http://127.0.0.1:8080`) |
| `crates/driver-influxdb/` | `dbcrust-driver-influxdb` (`InfluxClient`, `DatabaseType::InfluxDB`): **no InfluxDB crate** (the `influxdb` crate wraps reqwest 0.13 + aws-lc and has no Flux/v3), the three HTTP APIs are spoken directly on the reqwest 0.12 (rustls/ring) the HTTP drivers share, plus `csv-core` for the CSV streams. The URL scheme picks the generation and language (`influxdb://` v1 InfluxQL, `influxdb2://` v2 Flux, `influxdb3://` v3 SQL; trailing `s` / `+https` = TLS; `?version=`, `?language=influxql`); the API token lives in the password slot (`?token=` is moved there), so the secret-store binding, redaction and sessions treat it as a password. `conn` owns the client, auth headers, and the body reader (Ctrl-C/timeout polling, row/byte budget that drops the connection), `influxql` decodes chunked JSON, `flux` the annotated CSV, `csv` the incremental records, `exec` builds one request per statement (reads go `GET /query`, anything the classifier calls a write goes `POST`; InfluxDB 1.x does NOT enforce the verb — a write sent as GET still runs with a deprecation warning, verified on 1.8 — so read-only is classifier + read-only credential), `metadata` browses with `SHOW …` / the Flux `schema` package / `information_schema`. Classification lives in `crates/core/src/safety/influxdb.rs` (InfluxQL/SQL lexer + Flux import allowlist and write-function denylist); the broker refuses InfluxDB enrollment (`unsupported_engine`). Fake-server tests need no InfluxDB; real-server tests gate on `INFLUXDB_URL` (v1) and `INFLUXDB2_URL` (v2) |
| `crates/aws/` | `dbcrust-aws` (lib `dbcrust_aws`; shared AWS foundation for the AWS connectors, **not a driver**; **`rust-version = "1.94.1"`** in its own manifest, the workspace stays on 1.91): `settings` (`AwsSettings::from_connection_info`: region/profile/endpoint/path style/anonymous/static keys/role/timeouts; `credential_source()` is a pure function — anonymous > static > explicit profile > default chain; contradictions are errors), `config` (`load_sdk_config`: the loader, explicit region = `?region=` > explicit profile's > SDK default chain — never build `RegionProviderChain::or_default_provider()` by hand, its IMDS leg panics without an HTTP client —, `role_arn` via `AssumeRoleProvider`), `http` (`ring_client`: rustls **on ring only** via `aws-smithy-http-client` `rustls-ring`, optional `ca_cert`; every AWS crate is `default-features = false` because the SDK defaults pull aws-lc-rs, and this client is injected into each config — a build without it fails at the first request with "a http_client is required"), `redact` (`redact_error`/`redact_message`: error chains with secrets and `AKIA…`/`ASIA…` ids masked). Features `sso`, `credentials-process` (runs a command named by a config file: leave it off in a broker build). Tests inject `Env`/`Fs`/profile contents through `aws-config`'s `test-util` (dev-dependency): nothing reads the real `~/.aws` or environment |
| `crates/driver-dynamodb/` | `dbcrust-driver-dynamodb` (`DynamoClient`, `DatabaseType::DynamoDB`, **opt-in**: feature `dynamodb`/`aws`, `rust-version = "1.94.1"`): URL grammar shared with CloudWatch in `crates/core/src/database.rs::parse_aws_regional_url` (`dynamodb://[KEY:SECRET@][region][/table]`; a region-shaped host is the region, another host a custom HTTPS endpoint, `dynamodb+http://` plain HTTP, `?endpoint=`; `KEY:SECRET@` with no host uses the `_` placeholder host, see `escape_aws_empty_authority`; `DatabaseType::is_aws()` drives the S3-style secret binding/session code in `src/config.rs`). `ops` (ListTables, DescribeTable cached as `TableInfo` for key typing, Scan/GetItem/Query, PartiQL `ExecuteStatement`; every request has a `Limit` <= the rows still wanted and a `max_pages` budget, `ReturnConsumedCapacity=TOTAL` summed into a notice, a scan always warns; key text converted to the key attribute type and checked against the key schema before any read), `value` (AttributeValue -> cell: exact numbers, base64, JSON for M/L/sets, NULL type and missing = SQL NULL; columns = union with keys first, items' own names alphabetical, >200 fold into `_other`), `errors`, `metadata`. **Writes do not exist** (not even without `--read-only`). No automatic `LIMIT` for PartiQL (`add_engine_default_limit`). Broker: refuses DynamoDB (`unsupported_engine`). Offline tests against an in-process JSON-1.0 server; live test gates on `DYNAMODB_TEST_URL` (`docker run -p 8000:8000 amazon/dynamodb-local`, `dynamodb://AKIAFAKE:fakesecret@us-east-1?endpoint=http://127.0.0.1:8000`) |
| `crates/driver-cloudwatch/` | `dbcrust-driver-cloudwatch` (`CloudWatchLogsClient`, `DatabaseType::CloudWatchLogs`, **opt-in**: feature `cloudwatch`/`aws`, `rust-version = "1.94.1"`): `cloudwatch://[KEY:SECRET@][region][/group-or-prefix]` (the path is `database`; `/aws/…` may omit its slash; an exact group name wins over prefix matches; at most 50 groups per query). `ops::insights` = StartQuery -> GetQueryResults polled with a backoff -> rows (`results`: first-seen columns, `@ptr` hidden, duplicate names `_2`) with **StopQuery on every path that leaves the query running** (Ctrl-C, timeout, `max_scan_gb` budget on `bytesScanned`, failing poll; sent outside `guard_until` because the interrupt flag is exactly why we stop), a mandatory window (`resolve_window`: default `?since=` 1h, `?max_window=` 7d, refused before any request), `limit` = `max_rows` (<= 10000), statistics notice. Commands `groups`/`streams`/`tail` (FilterLogEvents, oldest first, no follow)/`use`. Read-only by construction (no write API reachable). Broker: refuses it. Offline tests against an in-process JSON-1.1 server only (LocalStack community cannot run Insights) |
| `crates/driver-s3/` | `dbcrust-driver-s3` (`S3Client`, `DatabaseType::S3`, **opt-in**: feature `s3`/`aws`, not in `bundled`/`standard`; `rust-version = "1.94.1"`): URL `s3://[KEY:SECRET@]bucket[/prefix]` (+ `r2://account/bucket`, `minio://host:9000/bucket` conveniences; parsed in `crates/core/src/database.rs::parse_s3_url`: bucket = `database`, prefix = `file_path`, custom endpoint = `host`/`port`/`use_tls`, AWS = no host, transport kept in the `tls` option); the secret key is the password slot, `session_token` a secret option, the binding's tls mode is `"<tls>@<region>"`. Commands only (`s3_command`), read-only by construction: `ops` (ListObjectsV2 paginated within `max_rows`/`max_pages`, HeadObject, ranged GetObject — a `Range` on every call, `max_get_bytes` hard cap, body read stops at the cap even if the server ignores `Range`, ListBuckets; every call raced against Ctrl-C/timeout), `format` (text/hex/base64 rows), `errors` (S3 code + hint + request id; HEAD answers have no body, so the status decides; wrong region names the right one), `metadata` (`\l` buckets, `\dt` prefixes+objects at the current prefix, `\d` HeadObject). Broker: refuses S3 (`unsupported_engine`, IAM cannot be proven). Not in the GUI form (`connectable_engines()` lists only compiled engines). Tests run the real SDK + ring client against an in-process HTTP server (no test-util: it adds ~15 crates); live test gates on `S3_TEST_URL` (MinIO: `docker run -p 9000:9000 minio/minio server /data`) |
| `crates/driver-duckdb/` | `dbcrust-driver-duckdb` (`DuckDbClient`, `DatabaseType::DuckDB`): the embedded DuckDB engine (`duckdb` =1.10506.1 = engine 1.5.6, `bundled` + statically linked `parquet`/`json`; a C++ build, about +20 MB → **opt-in feature `duckdb`, not in `bundled`/`standard`**, excluded from i686). The `duckdb` crate is synchronous and `Connection` is `!Sync`, so `conn` runs one dedicated OS thread per session that owns the connection and takes jobs over a channel; the async side polls the Ctrl-C flag/deadline and calls `InterruptHandle::interrupt()` (the session survives), results are streamed with `stream_arrow` + `step()` and cut at `max_rows` (default 100 000), `value` renders Arrow cells the way DuckDB prints them (nested → JSON text, exact DECIMAL/HUGEINT, BLOB `\xAB`, TIMESTAMPTZ in UTC, interval words), `metadata` browses `duckdb_*()` catalog functions of the **current database** with bound parameters only (`\l` = attached databases, `\c` = `USE`). `--read-only`/broker = the hardened profile built in `OpenSpec::config`: `access_mode=READ_ONLY` + `enable_external_access=false` + `lock_configuration=true` + autoinstall/autoload/community/unsigned extensions off (`allowed_directories` goes through `lock_down` SQL because it cannot be set in the config); an in-memory database cannot be read-only (error). Extensions are never downloaded without `install_extensions=true`. `ducklake` attaches a DuckLake catalog (`ducklake.rs`; refused when hardened). Textual guard: `crates/core/src/safety.rs` (`check_duckdb`, `Dialect::DuckDb` for the broker classifier); the broker accepts **local files, read-only only** (canonical path binding, re-checked at connect). Tests are real in-process (temp files); live DuckLake with `DUCKLAKE_TEST=1` |
| `third_party/tiberius/` | vendored `tiberius` 0.13.0 + the ring-only patch and a 32-bit build fix, **not a workspace member** (`exclude` in `[workspace]`, skipped by the `hk.pkl` fmt/clippy/whitespace steps); `DBCRUST_PATCH.md` has the source checksum, the diff, re-vendoring steps and the upstream proposal. Never edit it beyond that patch |
| `crates/driver-white-dragon/` | `dbcrust-driver-white-dragon` (`WhiteDragonClient`): SQL/qualifier HTTP adapter with generic WDSP schema introspection; may depend on `dbcrust-core` and its SDK (reqwest) only |
| `crates/driver-postgres/` | `dbcrust-driver-postgres` (`PostgreSQLClient`): sqlx `postgres` + pgvector; `format_postgresql_value` lives here; the global complex/vector display configs stay core statics |
| `crates/driver-mysql/` | `dbcrust-driver-mysql` (`MySqlClient`): sqlx `mysql`; the `.cargo/audit.toml` RSA exception covers this crate's sqlx-mysql |
| `crates/driver-clickhouse/` | `dbcrust-driver-clickhouse` (`ClickHouseClient`): `clickhouse` crate exactly pinned to `=0.15.1` (in `[workspace.dependencies]`; see Rust dependency policy) + reqwest for the HTTP calls |
| `crates/driver-elasticsearch/` | `dbcrust-driver-elasticsearch` (`ElasticsearchClient`): `elasticsearch` SDK (`elasticsearch::http` is an SDK module) + regex (default features: `(?i)` needs `unicode-case`) |
| `crates/driver-mongodb/` | `dbcrust-driver-mongodb` (`MongoDBClient`): `mongodb` + `bson` SDKs |
| `crates/driver-files/` | `dbcrust-driver-files` (`DataFusionClient`, serves Parquet/CSV/JSON; DuckDB is **not** DataFusion's anymore): Apache DataFusion (`datafusion::arrow`, `datafusion::sql::sqlparser` re-exports; `sqlparser` stays a single 0.61.0 shared with core) |
| `src/drivers.rs` | `pub use dbcrust_drivers::*;` — the registry facade; `database::create_database_client` re-exports the factory |
| `crates/core/src/model.rs` | Metadata DTOs (`TableDetails`, `ColumnInfo`, `IndexInfo`, …, `ColumnFilteringInfo`, `metadata_table_parts`); re-exported from `db` |
| `crates/core/src/explain_plan.rs` | EXPLAIN plan tree + `parse_postgresql_plan` (UI-independent; `explain_tui` and the GUI re-use it) |
| `src/sql_validator.rs` | Reedline `SqlValidator` (split from `sql_buffer`, which stays a pure lexer/splitter) |
| `crates/core/src/options.rs` | Connection-option keys DBCrust interprets (vault, `password_command`, `OPT_HARDEN_*`) and their classifiers; leaf module re-exported from `config`/`database` |
| `crates/core/src/output_format.rs` | `OutputFormat` (`-o/--format`); leaf, re-exported from `cli` |
| `crates/core/src/docker_target.rs` | `DockerTarget` + `PINNED_DAEMON_OPTION`: pure `docker://` URL parse/render/convert; re-exported from `docker` |
| `src/database_datafusion.rs` | File format queries (Parquet, CSV, JSON) |
| `src/ssh_tunnel.rs` | SSH tunnel management |
| `crates/docker/` | `dbcrust-docker` (lib `dbcrust_docker`, layer 1: core + bollard only): `docker` (`DockerClient`, `DockerError`, discovery; `From<DockerError> for DatabaseError` lives here), `docker_endpoint` picks the daemon (contexts), `docker_route` probes routes (published port → OrbStack domain → container IP), `docker_tunnel` + `docker_relay.pl` relay through `docker exec` when none answers. It never names `Database`: the caller passes the handshake as a closure (`connect_container`). `DockerTarget`/`PINNED_DAEMON_OPTION` stay in core. Root re-exports the four modules (`dbcrust::docker::…`) and keeps `Database::connect_docker_container` + its tests in `src/db.rs` |
| `src/vault_client.rs` | HashiCorp Vault credentials |
| `src/named_queries.rs` | Parameterized queries (`$1`, `$*`, `$@`) |
| `gui/core/` | `dbcrust-gui-core`: desktop app backend without GPUI — per-connection workers, `ConfigStore`, query/result/plan models |
| `gui/src/` | `dbcrust-gui`: GPUI Kit desktop app (views, workspace, `AppCommand` catalogue); module map in `gui/ARCHITECTURE.md` |

## Connection URL Types

```
postgres://user:pass@host:5432/db?sslmode=require
mysql://  sqlite:///path  mongodb://  clickhouse://
white-dragon://host:7700 # White Dragon search SQL over HTTP
redis://host:6379/0     # Redis/Valkey commands; rediss:// = TLS, redis+unix:///path = socket
libsql://host[:443]/ns?authToken=…  # libSQL/Turso over HTTPS (turso:// too); libsql+http://localhost:8080 = plain, loopback only (else ?tls=false); the token becomes the password
redshift://user:pass@cluster.xxx.region.redshift.amazonaws.com:5439/dev  # Amazon Redshift (redshift+tls:// too); TLS required by default, ?sslmode=verify-full&sslrootcert=…; no IAM yet
dynamodb://[KEY:SECRET@]eu-west-1[/Table]  # Amazon DynamoDB (opt-in `--features aws`/`dynamodb`, Rust ≥ 1.94.1); dynamodb+http://localhost:8000?region=local = DynamoDB Local, ?endpoint=, ?profile=, ?consistent_read=true; PartiQL SELECT + tables/describe/scan/get/query, read-only by construction
cloudwatch://[KEY:SECRET@]eu-west-1[/aws/lambda/group]  # Amazon CloudWatch Logs (opt-in `--features aws`/`cloudwatch`); ?profile=, ?since=1h, ?max_window=7d, ?max_scan_gb=, ?groups=a,b; Logs Insights over a mandatory window (`since 30m fields … | filter …`, `between … and …`) + groups/streams/tail/use; StopQuery on Ctrl-C/timeout
s3://[KEY:SECRET@]bucket[/prefix]?region=…  # Amazon S3 (opt-in `--features aws`, Rust ≥ 1.94.1); r2://account/bucket, minio://host:9000/bucket?tls=false, s3://bucket?endpoint=https://host; ?profile=, ?anonymous=true, ?path_style=, ?ca_cert=; commands ls/head/get/buckets/cd/pwd/help, read-only by construction
influxdb://user:pass@host:8086/db  # InfluxDB v1 (InfluxQL); influxdb2://:TOKEN@host/bucket?org=acme = v2 (Flux), influxdb3://:TOKEN@host/db = v3 (SQL); trailing s or +https = TLS; ?verify_certs=false, ?ca_cert=, ?language=influxql
duckdb:///path/file.duckdb  # DuckDB (opt-in feature): file or duckdb://:memory:; .duckdb/.ddb inferred; ?access_mode=read_only, ?memory_limit=, ?threads=, ?max_rows=; DuckLake: duckdb://:memory:?ducklake=sqlite:/meta.sqlite&data_path=…&install_extensions=true (never auto-downloads)
mssql://user:pass@host:1433/db  # SQL Server/Azure SQL (sqlserver:// too); TLS verified by default, ?encrypt=false|strict, ?trustServerCertificate=true (explicit opt-in), host%5CINSTANCE or ?instance= for named instances
session://name          # saved session
recent://               # recent-connection picker
file://                 # compatible-file picker in current directory
docker://container/db
vault://role@mount/database  # optional vault_credential_process runs lazily
path/to/file.csv        # infer CSV/Parquet/JSON/SQLite from extension
parquet:///path/*.parquet
csv:///path/file.csv?header=true&delimiter=,
json:///path/file.json
```

## Critical Rules (always follow these)

### 1. Command System — use strum, never hardcode lists

```rust
// CORRECT: strum EnumIter drives all command lists automatically
#[derive(EnumIter)] pub enum CommandShortcut { Q, H, Dt, ... }
pub fn get_command_names() -> Vec<&'static str> {
    CommandShortcut::iter().map(|s| s.command()).collect()
}

// WRONG: hardcoded vec misses new commands
vec!["\\q", "\\h", "\\l"]
```

Never remove the `strum` crate — it powers automatic synchronization across autocomplete, help, and Python CLI parity.

### 2. User Interactions — always use `inquire`

ALL interactive prompts MUST use the `inquire` crate (`Select`, `MultiSelect`, `Confirm`, `Text`). Never use raw `stdin`/`stdout` or `println!` for prompts.

### 3. Config — `#[serde(default)]` + `save_with_documentation` + `FieldSpec`

- New config fields require `#[serde(default)]` for backward compatibility.
- Always update `save_with_documentation()` in `src/config.rs` so the field appears with comments in generated configs. Root-level keys must be written **before** the first `[table]` section or TOML re-parents them.
- Add a `FieldSpec` entry to the schema in `src/config_editor.rs` (powers `\config` menu/get/set). The sync tests (`test_schema_matches_serialized_config`, `test_documented_save_covers_schema`) fail if either is missed.
- Persist config with `save_with_documentation()`, never plain `save()` (it strips all comments from the user's file).

### 4. Storage separation

User data lives in dedicated files, not mixed into `config.toml`:

| Data | File |
|------|------|
| App settings | `~/.config/dbcrust/config.toml` |
| Saved sessions | `~/.config/dbcrust/sessions.toml` |
| Recent connections | `~/.config/dbcrust/recent.toml` |
| Named queries | `~/.config/dbcrust/named_queries.toml` |
| Vault credentials | `~/.config/dbcrust/vault_credentials.enc` |
| Saved passwords, AI keys, tokens | OS keychain (service `dbcrust.store`) or `~/.config/dbcrust/secrets.enc` (Argon2id passphrase file); non-secret inventory in `secrets_index.toml` |
| Legacy passwords (read-only; `dbcrust secrets migrate`) | `~/.dbcrust` (format: `db_type:host:port:db:user:pass`), `ai_keys.enc` |

### 5. Testing

- Use `rstest` for parameterized tests.
- Tests never touch the real keychain/keyutils/`$HOME`: in test mode `secrets::default_store()` is an in-memory store; inject others with `secrets::with_store_override`. Real-keychain tests only behind `DBCRUST_TEST_KEYCHAIN=1`.
- `cfg!(test)` is only true inside the crate under test, so `dbcrust-secrets` sees every other crate's tests as a normal build. A crate whose tests can reach config/secrets (root, gui, gui-core) must enable the secrets crate's `test-mode` feature from its `[dev-dependencies]`; `src/test_isolation.rs` is the canary (run it with `RUST_TEST_MODE`/`DBCRUST_CONFIG_DIR` unset).
- All Rust tests are inline `#[cfg(test)]` modules — there is no `tests/` directory.
- Tests auto-isolate config to `/tmp/dbcrust_test_{pid}/` (heuristic: thread name contains "test" or `RUST_TEST_MODE` is set — `cargo test -- --test-threads=1` runs on the main thread and escapes it; export `RUST_TEST_MODE=1` in that case).
- PostgreSQL integration tests gate on `DATABASE_URL` env var and skip gracefully; other backends have no DB-gated tests yet.
- Python: `mise run py:test` runs `python/dbcrust/django/tests` (pure Python — the repo-root `conftest.py` stubs the native module when it isn't built).

### 6. Error handling

- `thiserror` for custom error types.
- PostgreSQL type decode: use `.or_else(|_| handle_custom_postgresql_type(...))` fallback, not `.map_err(...)`.

## Cargo features

Root `default = ["bundled"]` ships every engine and add-on (owner decision D3); `standard` = `postgres` + `mysql` + `sqlite`. Engine features (`postgres`, `mysql`, `sqlite`, `mongodb`, `clickhouse`, `elasticsearch`, `white-dragon`, `redis`, `mssql`, `redshift`, `libsql`, `influxdb`, `files`, and the opt-in `duckdb`) forward to `dbcrust-drivers/<x>`; add-ons are `docker`, `ai`, `explain-tui`, `clipboard`. **Opt-in (not in `bundled`, not in `standard`)**: `duckdb` (a C++ engine: `cargo build --features duckdb`, ~15 CPU-minutes cold, +19 MB release; excluded from i686 and from wheels/releases, own path-filtered workflow `duckdb.yml`; mbx cannot cache its build script, so use `CARGO_TARGET_DIR=.target-duckdb` with plain cargo while hacking on it); `s3`, `dynamodb`, `cloudwatch` and their umbrella `aws = ["s3", "dynamodb", "cloudwatch"]`; they need Rust ≥ 1.94.1 (the AWS crates' `rust-version`, set per crate; CI job `rust-aws` pins that toolchain) and `cargo tree --locked --workspace --target all -i aws-lc-rs` must stay empty. `DatabaseType` variants and config parsing never depend on features.

- Gate call sites with `#[cfg(feature = "…")]`; a disabled add-on answers `crate::features::FeatureNotCompiled`, a disabled engine `DatabaseError::ConnectorNotBuilt`. Commands keep their `Command` variants (so `\ai`/`\docker` explain themselves instead of "unknown command").
- Hide disabled engines from completion/help with `dbcrust_drivers::is_compiled` / `UrlScheme::is_available`; never hardcode lists.
- **New connector** = driver crate + optional dep and feature in `dbcrust-drivers` + one `connectors!` row + root feature forwarding to it + a decision on `bundled`: light connectors are added to `bundled` explicitly (changelog note), heavy ones (DuckDB, AWS) stay opt-in. Add it to the CI matrix (`rust-features`) and `mise run check:features`.
- Verify with `mise run check:features` (zero-engine, `standard`, each feature alone) and `cargo test -p dbcrust --no-default-features --features standard`; tests that need a disabled engine/add-on are `#[cfg]`-gated, not deleted. Release binaries, wheels and gui-core use `bundled` explicitly.

## Rust dependency policy

- Do not run blanket dependency upgrades. Review release notes, provenance/maintainer changes, RustSec advisories, and newly introduced transitive crates; keep breaking migrations separate.
- New crate releases must be at least **24 hours old**, including security updates. Use `cargo update -p CRATE --precise VERSION`, then `python3 .github/scripts/check_cargo_cooldown.py --base HEAD` before compiling (Python 3.11+). CI checks lockfile additions against the PR base, including transitive and target-only entries, with no security-PR exemption.
- Use `--locked` for validation/builds. Run `cargo audit`, `cargo machete --with-metadata`, and `cargo +nightly udeps --workspace --all-targets --all-features --locked` when reviewing removals. `prettytable-rs` is imported as `prettytable`; never remove it based on a name-only false positive.
- `.cargo/audit.toml` documents the narrowly justified SQLx/MySQL RSA exception; do not suppress additional advisories to make CI green. See [dependency review notes](docs/src/content/docs/user-guide/development.md#rust-dependency-updates) for residual upstream warnings.
- `tiberius` is **vendored** in `third_party/tiberius` (0.13.0 + a small patch: `tokio-rustls` without default features + `ring`, so no `aws-lc-rs`; and `usize::MAX - 1` for the XML size literal that overflows on i686): upstream hard-codes aws-lc, does not compile for 32-bit, and 0.12.3 carries RUSTSEC-2026-0104/0098/0099. Re-vendor on a release as `DBCRUST_PATCH.md` says, keep `cargo tree --locked -i aws-lc-rs` empty, and drop the copy once upstream has a ring feature. Its registry dependencies still go through the cooldown script and `cargo audit`.
- ClickHouse is exactly pinned to **0.15.1**: 0.15.2's unconditionally compiled Native reader overflows a `usize` constant on i686. Keep the pin until an upstream fix is reviewed and **Rust · 32-bit Python compatibility** passes. Confirm that CI check before tagging dependency releases; a successful 64-bit build does not validate our 32-bit Python wheels.

## Adding a New Feature Checklist

- [ ] Core logic in appropriate module
- [ ] New `Command` variant + `CommandShortcut` variant (strum handles the rest)
- [ ] Config field with `#[serde(default)]` + `save_with_documentation` entry
- [ ] `inquire` for any interactive prompts
- [ ] Unit tests in the module's `#[cfg(test)]` block
- [ ] `cargo test && cargo clippy` clean
- [ ] Update `docs/src/content/docs/reference/backslash-commands.md` if new `\cmd` added

## Python CLI

Python CLI calls Rust directly via PyO3 — zero separate implementations. `run_cli_loop(args: Vec<String>)` in `src/lib.rs` is the entry point. All connection URL types and commands work identically. Compile with `--features python`.

## Logging

Configure in `~/.config/dbcrust/config.toml`:

```toml
[logging]
level = "debug"          # trace | debug | info | warn | error
console_output = true
file_output = false
file_path = "~/.config/dbcrust/dbcrust.log"
```

## SSH Tunnels

```toml
[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "user@jumphost.example.com:2222"
```

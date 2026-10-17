# DBCrust: secure agent connections, workspace boundaries, and connector expansion

Research and implementation plan, 2026-10-15. **No implementation or commit in this round.**

Repository examined: `cb73e5ec19eba30820ba9342b00674e3f9aabc90`. dbflux examined at `de08d2036ff5ae8b6e83b67f794669aab9a4904f` (its `main` when fetched). References to local `file:line` refer to this DBCrust revision. Read `AGENTS.md` and `CLAUDE.md` before research. No real user credential files or keychain items were read, decrypted, changed, or tested.

## Recommendation

Ship **named, policy-bound connections backed by secure storage**, followed by a **credential-owning broker with an alias-only MCP/CLI client**. Make PostgreSQL with a dedicated least-privilege database role the first production-supported agent backend. Move existing secrets off machine-derived encryption immediately; do not make that fix wait for a workspace rewrite.

Offer two explicitly different deployment assurances:

1. **Local convenience mode:** OS keychain, no credential exports, redacted outputs, alias-only queries, immutable-in-session policies and separate human approval. Prevents routine credentials entering agent context, CLI arguments, history, or MCP responses. It is not a boundary against arbitrary same-user shell execution.
2. **Isolated production mode:** broker, credential store, authoritative target/policy records and approval endpoint outside the agent's OS identity/sandbox. Agent receives only a narrowly scoped query capability and access to the query socket; no broker filesystem, keychain/session bus, debugger, administrative endpoint, Docker socket, or direct production credential source. This is the recommended deployment for the owner's literal hostile-agent requirement. A separate service account or an agent VM/container with an appropriately restricted socket bridge is an actual boundary; merely starting another process as the same user is not.

Split by dependency and security boundaries, not by source-file count. Preserve the root `dbcrust` package as the compatibility facade, both CLI binaries, and the maturin extension. Extract driver contracts, storage, secrets, execution/policy, CLI and drivers incrementally. Keep current full functionality during extraction; offer a small PostgreSQL/MySQL/SQLite preset and an explicit full preset. Add new connectors as opt-in crates, with Redis and SQL Server first, Redshift as a small PG-wire adaptation, then HTTP/cloud/embedded engines.

## A. What dbflux actually provides

### Sources and corrections

Sources inspected directly over HTTPS (source retrieval, not a clone/build):

- [Repository and workspace manifest](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/Cargo.toml), [`crates/dbflux/Cargo.toml`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/crates/dbflux/Cargo.toml).
- [`ARCHITECTURE.md`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/ARCHITECTURE.md), [`docs/DRIVERS.md`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/docs/DRIVERS.md).
- [`docs/DRIVER_RPC_PROTOCOL.md`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/docs/DRIVER_RPC_PROTOCOL.md), [`framing.rs`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/crates/dbflux_ipc/src/framing.rs), [`transport.rs`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/crates/dbflux_driver_ipc/src/transport.rs), [`auth.rs`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/crates/dbflux_ipc/src/auth.rs).
- [`docs/MCP_AI_INTEGRATION.md`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/docs/MCP_AI_INTEGRATION.md), [`governance.rs`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/crates/dbflux_mcp_server/src/governance.rs), [`PRIVACY.md`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/PRIVACY.md).
- [`secrets.rs`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/crates/dbflux_core/src/storage/secrets.rs), [`secret_manager.rs`](https://github.com/0xErwin1/dbflux/blob/de08d2036ff5ae8b6e83b67f794669aab9a4904f/crates/dbflux_core/src/storage/secret_manager.rs), driver manifests for PostgreSQL, Redis, SQL Server and InfluxDB.

The pinned manifest lists **53 workspace members, including four vendored gpui-mcp members**, rather than the reported 60+. DBCrust has **three packages** today, with the root still the monolithic engine library, and **73,519 Rust source lines under `src/` including inline tests**, rather than approximately 65k. These distinctions matter when sizing the split.

### Connector comparison and recommended implementation

Weight means *expected additional build/dependency cost compared with existing dependencies*, not measured per-connector MB. L = reuse/light HTTP adapter; M = new protocol/SDK; H = substantial SDK/codegen; VH = embedded engine/toolchain. Actual marginal costs must be measured with identical feature/profile builds after extraction. P0 = existing functionality/security, P1 = first expansion, P2 = next wave, P3 = optional larger investment.

| Engine | dbflux implementation / scope | DBCrust today | Proposed DBCrust crate and library | Weight / priority |
|---|---|---|---|---|
| PostgreSQL | `postgres` + `postgres-native-tls`; rich metadata, cancellation | SQLx PG, metadata, vector/custom types | `dbcrust-driver-postgres`: keep SQLx PG, pgvector | Existing H / P0 |
| MySQL/MariaDB | `mysql`; two registrations sharing implementation | SQLx MySQL, MariaDB-compatible protocol | `dbcrust-driver-mysql`: SQLx MySQL; separate MariaDB metadata if tested | Existing M / P0 |
| SQLite | bundled `rusqlite` | SQLx SQLite | `dbcrust-driver-sqlite`: keep SQLx SQLite; do not add a second SQLite linkage | Existing M / P0 |
| Redshift | separate PG-wire driver; read-only; no IAM/SSO, DDL or multi-statement | PG wire may connect, but no dedicated introspection or supported product contract | `dbcrust-driver-redshift`: shared PG transport helper, SQLx PG; Redshift-specific catalog/type queries | L / P1 |
| SQL Server/Azure SQL | `tiberius` 0.12, Tokio compatibility, rustls, SQL Browser | None | `dbcrust-driver-mssql`: `tiberius` + `tokio-util/compat`; SQL auth first, named instances explicit, Windows integrated auth later | M / P1 |
| MongoDB | `mongodb`, document operations; no cancellation | `mongodb` 3.9, BSON 2; native shell-like subset + SQL translation | `dbcrust-driver-mongodb`: retain current client, expose document capabilities honestly | Existing H / P0 |
| Redis | `redis` 0.27; key/value, streams, TLS, cluster/sentinel; no cancellation | None | `dbcrust-driver-redis`: reviewed async `redis`, minimal Tokio/rustls features; SCAN and bounded reads first | M / P1 |
| DynamoDB | `aws-sdk-dynamodb`; native envelopes; no PartiQL/transactions | None | `dbcrust-driver-dynamodb`: AWS SDK, typed scan/query/get operations; no write support until policy classification | H / P2 |
| InfluxDB v1/v2 | `reqwest` HTTP; InfluxQL, v2 Flux; read-only | None | `dbcrust-driver-influxdb`: existing async reqwest, annotated CSV decoder; explicit dialect/version and bounded results | L–M / P2 |
| ClickHouse/Cloud | reqwest JSON HTTP; read-oriented, no structured DDL/mutations or generic parameters | `clickhouse =0.15.1` plus HTTP execution | `dbcrust-driver-clickhouse`: retain exact pin; test HTTPS Cloud, quotas, cancellation separately | Existing M / P0 |
| Turso/libSQL | `turso_serverless =0.1.3`, Hrana HTTP; interactive streams; no replicas/cancel | None | `dbcrust-driver-libsql`: evaluate remote-only `turso_serverless` vs `libsql` remote feature in a spike; prefer HTTP-only dependency, no second SQLite C build | L–M remote / P2 |
| DuckDB/DuckLake | bundled `duckdb`, Parquet/JSON; DuckLake attachments; no statement timeout | **Not genuine DuckDB support.** Enum/scheme routes to DataFusion; registration rejects unsupported format | `dbcrust-driver-duckdb`: reviewed `duckdb`, feature-gated bundled engine; DuckLake extensions/catalogs a later subphase | VH / P3 |
| CloudWatch Logs | `aws-sdk-cloudwatchlogs`, Logs Insights/PPL/SQL, time/source context; no cancel yet in docs | None | `dbcrust-driver-cloudwatch`: AWS SDK; poll Start/GetQueryResults, StopQuery, query cost/time windows | H, lower marginal after AWS / P2 |
| S3/R2/MinIO | `aws-sdk-s3`; object browser/CRUD/presign; no multipart or S3 Select | DataFusion has `object_store` transitively, but no first-class S3 connector | `dbcrust-driver-s3`: prefer `object_store` with AWS feature for listing/read/DataFusion integration; add SDK only for unmet versions/presign/CRUD requirements | M–H / P2 read-only, P3 rich CRUD |
| External drivers | local IPC proxy + driver host; versioned capability/form discovery | None | `dbcrust-driver-rpc` + `dbcrust-driver-host`; trusted executable allowlist, versioned JSON protocol initially | L core, arbitrary worker weight / P3 |
| Elasticsearch | No dedicated engine in listed driver set (CloudWatch OpenSearch languages are different) | Elasticsearch SQL client | `dbcrust-driver-elasticsearch`: retain SDK initially; later assess existing reqwest for narrow SQL API | Existing M / P0 |
| White Dragon | No corresponding shipped driver | HTTP SQL/qualifier search | `dbcrust-driver-white-dragon`: reqwest | Existing L / P0 |
| CSV/JSON/Parquet SQL | file features, Parquet/Arrow crates; not DBCrust's DataFusion contract | DataFusion 53.1, Arrow/Parquet 58.1 | `dbcrust-driver-files`: DataFusion; one crate for three formats, independent of DuckDB | Existing VH / P0 opt-out first |

DuckDB evidence: `src/database.rs:694,860`, `src/database_datafusion.rs:448`. Do not advertise DuckLake until a real DuckDB adapter, extension installation policy, metadata catalog credentials and object-store access are implemented and tested.

### Adopt these architectural ideas

- Separate `DbDriver` (metadata, forms, connect) from live `Connection` (execute, metadata, transaction/cancel capabilities). DBCrust already has `DatabaseClient`/`MetadataProvider`; evolve them instead of replacing all drivers at once (`src/database.rs:1115,1245`). Use driver-owned capability and form schemas to keep CLI/GUI/MCP in agreement.
- Distinguish relational, document, key/value, time-series, log-stream and object-storage operations. Redis and S3 should not become misleading SQL adapters. Use optional subtraits and typed operation/result enums; unsupported means a typed error, never a silent fallback.
- Capabilities must distinguish **native enforcement**, **client heuristic**, and **unsupported** for read-only, timeout, cancellation, row/byte limits, parameter binding and transactions. A flag saying “supports read-only” is insufficient for production policy.
- Adopt secret references and `SecretString`-style explicit exposure, keyring error distinctions, bounded blocking keyring calls, and “save failure is a failure.” Avoid a no-op store that returns success on write. Use a bounded worker pool/circuit breaker, not indefinitely accumulating timed-out threads.
- Adopt governance on *every execution path*: trusted client, connection exposure, tool and operation classification, policy, approval, execution, audit. dbflux has Allow/Ask/Deny and single-use approval bound to actor/connection/tool/arguments. Its docs explicitly say most-permissive grants win; **DBCrust should use deny-overrides/intersection with connection ceilings**, so an added role cannot silently undo a production restriction.
- Adopt fingerprint-only audit defaults and always redact sensitive structured fields. Keep full SQL history distinct from audit; an audit privacy claim must not hide plaintext history elsewhere.

### RPC and MCP details worth learning from, not copying blindly

dbflux driver services use `interprocess` local sockets (Windows named pipes), Hello/version negotiation, request/session IDs, OpenSession/CloseSession, cancellation/chunk capabilities, metadata and form descriptions, and managed child lifecycle. The current wire implementation is **4-byte little-endian length + postcard**, capped at **16 MiB**. The protocol document's opening “bincode” sentence is stale; `framing.rs:8,18,24` and the later enum-index compatibility discussion establish postcard. Negotiate the highest mutually supported minor within a major and reject post-handshake version mismatches. Capability enforcement is essential: older RPC peers cannot enforce row limits; read-only/statement timeout are refused; a lying driver remains trusted code.

DBCrust should start with a deliberately small, language-neutral, length-bounded JSON driver protocol, stable string operation names and a conformance harness. Add binary row batches only after throughput measurements. No public plugin marketplace or arbitrary downloaded executables initially. External drivers that authenticate to a database necessarily see its credentials unless an authless proxy is provided: run approved workers inside the broker trust boundary, disable them for protected production aliases by default, and do not call process separation a sandbox.

dbflux MCP runs as a separate stdio server inside the app binary and loads shared SQLite profiles/governance/audit. The GUI is the approval surface; MCP cannot approve/reject its own calls. Approved arguments are matched and consumed once; expiration is 24 hours in dbflux. Prefer a shorter DBCrust approval TTL (5 minutes), policy/target revision binding and an at-most-once execution record. Client IDs are labels, not authentication. A user-readable shared IPC token and user-writable policy DB do not protect against the same user. dbflux privacy docs describe child environment tokens; current `auth.rs` replaces global environment mutation with a process-global store while transport retains environment fallback. Do not copy secret-bearing environment delivery.

Do not copy 53 crates, the GUI-heavy binary dependency graph, bundled DuckDB/AWS defaults, synchronous engine wrappers in an already-async DBCrust, Lua/JS/hooks, or the unified SQLite settings migration just for similarity. DBCrust's dedicated TOML files and `FieldSpec` contract stay. dbflux's `--config-dir` not relocating governance storage is specifically a pattern to avoid for test isolation.

## B. Measurements and build design

### Reproducible baseline

Host: macOS Darwin 25.6.0, `aarch64-apple-darwin`, 12 logical CPUs, 19,327,352,832 bytes RAM (18 GiB). Rust 1.96.0 (`ac68faa20`, LLVM 22.1.2), Cargo 1.96.0, mbx 1.4.1, cargo-bloat 0.12.1. `mise install` succeeded (tools already installed). `mbx doctor`: no failures/warnings, local cache, reflinks, 20 GiB budget, no remote. Worktree initially clean and had no `target/`; shared caches were warm. No dependency or lockfile edits. All Cargo commands ran through mise and used `--locked` where applicable. No DB connections/tests were needed for this plan.

Commands:

```sh
mise install
mise exec -- mbx doctor
DBCRUST_CONFIG_DIR=$(mktemp -d /tmp/dbcrust-plan-config.XXXXXX) RUST_TEST_MODE=1 \
  /usr/bin/time -l mise exec -- cargo build --locked --bin dbcrust --timings
# Timestamp-only touch of src/commands.rs, same command; restore original mtime afterward.
# No source bytes changed. A Python try/finally performed os.utime + subprocess.run.
DBCRUST_CONFIG_DIR=$(mktemp -d /tmp/dbcrust-plan-config.XXXXXX) RUST_TEST_MODE=1 \
  /usr/bin/time -l mise exec -- cargo build --locked --release --bin dbcrust --timings
mise exec -- cargo tree --locked -d --depth 0
mise exec -- cargo tree --locked -i reqwest@0.13.2 --depth 2
mise exec -- cargo tree --locked -e features -i aws-lc-rs --depth 5
```

| Measurement | Observed result | Interpretation |
|---|---|---|
| Fresh worktree dev build, absent target, warm mbx | **66.81 s wall**, 58.41 s user, 55.81 s system; max RSS **1,831,665,664 B** | Clean-ish checkout, **not** uncached cold compilation; macOS time RSS is per-process high-water, not aggregate whole-build memory |
| Cache report | **865 hits, 12 misses, 3 not looked up, 176 bypassed**; 324.5 MiB stored | Cargo “Compiling” lines include restored artifacts and cannot be interpreted as compiler work |
| Timestamp-only `commands.rs` rebuild | **49.214 s wall**, Cargo 48.58 s; max RSS **1,508,327,424 B**; **600 hits, 0 misses, 142 bypassed** | Actual requested touch workflow; warm cache but many dependencies replayed. Not a clean estimate of a semantic edit's incremental compiler latency |
| First dev timing | 601 units; root library **33.81 s** from t=27.42; binary **5.03 s** from t=61.23 | Root library/final linking dominate the tail even with dependency cache |
| Other long dev units | `aws-lc-sys` build script **16.74 s**, zstd-sys **5.89 s**, ring build script **5.52 s**, sqlx-sqlite **4.42 s** | Mixed native/build-script/cache-restoration path, not a claim that DataFusion compiles cheaply |
| Touch timing | root library **26.76 s** from t=21.42; AWS-LC build script **16.60 s** | Shared-cache/native replay needs its own diagnosis before attributing all delay to monolith size |
| Release build and binary (round 2, `cargo build --locked --release --bin dbcrust`) | **5 m 48 s wall** (110.6 s user, 52.2 s sys), max RSS 1.10 GB; mbx 860 hits / 2 misses; stripped `target/release/dbcrust` **32,266,000 B (30.8 MiB)** | Dependencies were cache hits, so nearly all wall time is the monolithic root crate under `codegen-units=1` + thin LTO + `opt-level="z"`. That is the strongest argument for the crate split (parallel codegen across crates) and for feature-gating heavy subsystems |

Timing artifacts: `target/cargo-timings/cargo-timing-20261015T124019230Z-c38b1c41837ceb23.html` and `cargo-timing-20261015T124219596Z-c38b1c41837ceb23.html`. Extracted embedded `UNIT_DATA` with Python `json.JSONDecoder().raw_decode`. They are generated/untracked build artifacts managed by mbx and may be garbage-collected; the numbers above are preserved here. The observed late dependency-to-root-to-bin path ends at 66.26 s; native TLS setup contributes before the root becomes runnable. These are wall intervals, not additive independent-crate compile estimates.

### Quick wins before a large split

1. **Gate DataFusion/files, AI, MongoDB, Elasticsearch, Docker, clipboard and explain TUI** behind additive optional features, initially enabled by a compatibility preset. A source move alone does not reduce final binary size or initial dependency work. A connector-disabled build must not contain its dependencies in `cargo tree`.
2. **Fix TLS feature unification deliberately.** `genai =0.7.0-beta.2` defaults to `rustls-tls`, which enables reqwest 0.13.2 `rustls` → AWS-LC. Both ring and AWS-LC are compiled today despite comments. Spike `genai/default-features=false` plus a deliberately configured reqwest 0.13 alias using `rustls-no-provider` and an explicitly installed ring provider; inspect whether genai accepts the shared client. Test CLI, Python and GUI runtime initialization. Do not simply disable TLS or change certificate trust behavior. This is a candidate, not an already-validated fix.
3. **Development debug/profile experiments:** measure `debug="line-tables-only"`, dependency `debug=false`, and retaining macOS unpacked debug info. Compare semantic edit/type-check, link time, disk and debugging usability. For developer optimized builds compare no LTO/codegen-units=16 against existing fast-release (which still uses thin LTO); retain shipping profile until size and query benchmarks justify a change. `opt-level=z` can trade query throughput for size; do not assume a universal win.
4. Duplicate versions found: reqwest **0.12.28/0.13.2** (0.13 only through genai); base64 **0.22.1/0.23.1**; strum **0.26.3/0.27.2/0.28.0**; syn **2.0.117/3.0.7**; hashbrown **0.14.5/0.15.5/0.16.1/0.17.1**; rand **0.8.8/0.9.5**; getrandom **0.2.16/0.3.4/0.4.2**; lz4_flex **0.11.6/0.13.0**; sha2 **0.10.9/0.11.0** plus related RustCrypto generations; unicode-width **0.1.14/0.2.0**; webpki-roots **0.26.11/1.0.2**. Some same-version duplicates are host/target/features, not removable version duplication. Prioritize optional AI over forcing a reqwest migration; do not patch incompatible versions to one number. Never remove strum. Treat base64 alignment as a small reviewed API change with tests, not automatic savings.
5. SQLx features are already largely explicit/default-disabled. Verify why migration/offline features are unified and whether pgvector/transitives require them; do not remove chrono/JSON/UUID/decimal/vector decoding support blindly. DataFusion already disables many default features; SQL pulls nested functions transitively, so removing a top-level feature name may not remove code.
6. Root `lib` emits both `rlib` and `cdylib`. Audit whether ordinary CLI builds unnecessarily link the cdylib; Cargo's library `required-features` is not a dependable way to gate crate types (the no-python build does build the library). Separating the extension *later* can save linking, but is not allowed to break `--features python` in the migration.
7. Investigate mbx repeated build-script/native work with `mise exec -- mbx explain`; keep this separate from application changes. Do not purge the shared cache to manufacture a cold baseline on the owner's machine. A future clean runner with recorded cache-off configuration is the right uncached benchmark.

Dependency review for any change: no blanket upgrades; releases including transitives/target-only additions must be at least 24h old; precise updates only; run `python3 .github/scripts/check_cargo_cooldown.py --base <PR-base>` before compiling new lockfile content. Keep **ClickHouse exactly 0.15.1** until upstream/i686 wheel checks pass. Review release notes, maintainer/provenance, RustSec. For removals use `cargo audit`, `cargo machete --with-metadata`, and `cargo +nightly udeps --workspace --all-targets --all-features --locked` through mise. Account for `prettytable-rs` → `prettytable` import. Existing SQLx/MySQL RSA exception is not license for additional suppressions. No removal or upgrade was made in this research.

### Concrete target workspace

Keep root default-member `.` and explicit workspace resolver `3` (edition 2024; verify feature behavior in CI). All internal dependencies `default-features=false`; share versions/features under `[workspace.dependencies]`. The final layout is approximately two dozen crates with existing drivers, growing one per new engine, not a goal of 60.

| Package / path | Owns | Must not depend on |
|---|---|---|
| `dbcrust` / root | Compatibility Rust reexports; `dbcrust`/`dbc` bins; feature forwarding; current PyO3 wrappers/module | GUI/GPUI |
| `dbcrust-model` / `crates/model` | Engine IDs, typed values/results, schema `TableDetails`/columns/indexes, safe public connection metadata, secret reference IDs, errors | SQLx, Arrow, drivers, TUI, config I/O |
| `dbcrust-driver-api` / `crates/driver-api` | Async driver/session/metadata traits, capabilities, `ConnectOptions`, operation and execution-budget contracts, ephemeral authentication input | Concrete drivers, application Config, frontend |
| `dbcrust-secrets` / `crates/secrets` | Secret wrappers, keyring/file provider, legacy migration reader, zeroization, explicit expose boundaries | CLI prompts, driver SDKs, AI |
| `dbcrust-storage` / `crates/storage` | Config DTOs, dedicated TOML files, atomic persistence, settings schema data; secret refs only | Driver implementations, prompting, raw credential lookup |
| `dbcrust-sql` / `crates/sql` | Existing SQL parser/context/regex translation + dialect helpers, statement splitting, query classification adapters | DataFusion, UI, concrete engine clients |
| `dbcrust-format` / `crates/format` | Typed-value rendering, JSON/CSV/table output, complex/vector/geo display, pure EXPLAIN analysis | Pools, credentials, frontend widgets |
| `dbcrust-policy` / `crates/policy` | Pure authorization, approval binding model, bounds, deny reasons | Keychain, DB clients, UI |
| `dbcrust-audit` / `crates/audit` | Bounded structured audit writer, correlation IDs, retention, fingerprinting | UI and connector SDKs |
| `dbcrust-connect` / `crates/connect` | Alias/target validation, secrets resolution orchestration, optional Vault/SSH/Docker adapters | CLI commands/TUI; never permit agent-supplied arbitrary credential helper |
| `dbcrust-drivers` / `crates/drivers` | Registry/composition root; optional dependencies on individual drivers | CLI/GUI, PyO3 |
| `dbcrust-runtime` / `crates/runtime` | `Database` facade, execution session, shared policy/audit entry point, cancellation/limits | CLI/reedline/GPUI |
| `dbcrust-cli` / `crates/cli` | Args, commands/strum, REPL, inquire, completions, history, config UI, optional explain TUI/pager/clipboard | GUI |
| `dbcrust-ai` / `crates/ai` | Optional genai, OAuth, internal investigation loop through authorized execution API | Own duplicate keystore or unrestricted driver access |
| `dbcrust-agent-protocol` / `crates/agent-protocol` | Versioned broker request/result/approval status DTOs; no credential DTO | Drivers/secrets |
| `dbcrust-agent-client` / `crates/agent-client` | Small optional `dbcrust-agent` binary, broker transport; stdio MCP adapter using reviewed `rmcp` | Drivers, keyring, storage migration, production raw credentials |
| `dbcrust-broker` / `crates/broker` | Credential-owning process, immutable request policy, query/admin endpoint separation, optional MCP server integration | REPL/pager/editor/AI, GPUI |
| `dbcrust-driver-{postgres,mysql,sqlite,mongodb,clickhouse,elasticsearch,white-dragon,files}` / `crates/driver-*` | Current adapters and inline tests, only their SDKs | Root facade, GUI, CLI, application storage |
| `dbcrust-gui-core` / `gui/core` | Existing workers and GUI models, calls runtime/storage | Root monolith after migration, CLI, PyO3 |
| `dbcrust-gui` / `gui` | Existing GPUI frontend | Driver SDKs or credential exposure |

Dependency direction (`A → B` means A depends on B):

```text
model ← driver-api ← driver-* ← drivers ← runtime ← cli ← root facade/bins/PyO3
model ← sql/format ──────────────────────┘         ↑
model ← storage/secrets ← connect ───────┘         ai (optional, execution interface)
model ← policy/audit ────────────────────┘
runtime + connect + policy + audit + agent-protocol ← broker
agent-protocol + MCP transport ← agent-client (no driver edge)
runtime + storage + format ← gui-core ← gui
```

Avoid cycles during extraction: `database.rs:1139` currently returns `crate::db::TableDetails`; move DTOs first. Driver code references `complex_display`, `PerformanceAnalyzer`, config option helpers and SQL parser (`database_white_dragon.rs:1044`); move pure pieces into lower crates or inject immutable snapshots. Replace global complex-display config with execution/render options. `src/database.rs:51` global read-only request and GUI construction gate (`gui/core/src/connect.rs:177`) become explicit per-session `ConnectOptions`, not a cross-connection global. Split `db.rs` rendering/TUI from execution; runtime must never import a frontend.

Feature contract:

```toml
# Illustrative final root forwarding; corresponding registry features own dep: edges.
[features]
default = ["standard"]
standard = ["postgres", "mysql", "sqlite", "ssh", "vault"]
bundled = ["standard", "mongodb", "clickhouse", "elasticsearch", "white-dragon",
           "files", "docker", "ai", "explain-tui", "clipboard"]
postgres = ["dbcrust-runtime/postgres"]
mysql = ["dbcrust-runtime/mysql"]
sqlite = ["dbcrust-runtime/sqlite"]
files = ["dbcrust-runtime/files"]
python = ["dep:pyo3", "pyo3/macros"]
```

During migration **default remains `bundled`** (equivalent to current behavior). Switch source-install default to `standard` only in an announced release after owner decision; ship full CLI/GUI/wheels with explicit `bundled` so existing users retain engines. New Redis/MSSQL/cloud/DuckDB features are opt-in and not silently added to `bundled`. Security/policy for protected aliases is never a compile-out escape hatch. No-default build can render help and return `ConnectorNotBuilt` for recognized-but-disabled engines. Engine identity enums remain stable for deserialization; strum-derived registration/help filters reflect compiled capabilities rather than hardcoded command lists. SQLx PG/MySQL/SQLite features belong to the corresponding driver dependencies; pgvector belongs only to PG, and Arrow/DataFusion types must not leak through public contracts.

Python compatibility: preserve `dbcrust._internal`, `#[pymodule]`, ABI3 Python 3.10, public Python classes and `run_command`/`run_cli_loop`, root `--features python`, and `pyproject.toml` maturin settings. First move binding source into a root module only. Keep compatibility reexports for `dbcrust::database`, `dbcrust::config`, `dbcrust::Database` while internals move. A later optional `dbcrust-python` cdylib packaging change is a separate compatibility decision, not a prerequisite for driver extraction. GUI switches to lower crates; temporary `gui-core::dbcrust` compatibility reexports must not pull CLI back in.

## C. Secret audit: current sources, persistence and sinks

This is a source-level inventory, not a penetration-test certificate. **Confirmed** means the source directly implements the flow; **risk** means reachable unsafe types/unfiltered sink without proof that a real password was emitted in a particular runtime. No actual secrets were accessed. Ordinary query data can contain credentials; hiding connection credentials cannot make arbitrary returned data safe.

| Surface and references | Finding | Required action |
|---|---|---|
| `src/password_encryption.rs:24,89,120,131,157` | **Confirmed:** AES-GCM key is SHA-256 of machine/user/home/hostname identifiers and a fixed salt; plaintext passthrough without `enc:`. Encryption is not protection from the local user; hardware/hostname changes can also break recovery | Stop new writes, retain explicit legacy migration reader only; random key/passphrase fallback |
| `src/dbcrust_pass.rs:66,98,135,155,264,310,387` | **Confirmed:** decrypted `String` with derived Debug; wildcard host/port/db/user matches; `encrypt=false` permits plaintext; env-selected path; truncate/write then chmod 0600; Windows permission check always succeeds | Secret wrapper, exact target bindings for protected aliases, no new plaintext save API, secure atomic writer/Windows ACL, legacy wildcard import review |
| `src/vault_encryption.rs:20,37,53` | **Correction:** cache key is SHA-256(Vault token + salt), not machine ID. Token comes from explicit auth, `VAULT_TOKEN`, or `~/.vault-token` | Migrate cache encryption under new provider or disable disk cache for protected dynamic credentials; same-user token possession defeats old cache encryption |
| `src/config.rs:353,368,1338,1377,1390,3289` | **Confirmed:** decrypted Vault password structs are cloneable/Debug; cached TOML encrypted on disk; loader deletes unreadable cache except missing-token case; list API returns credentials | Public metadata-only cache view; migration must not invoke destructive loader; retain unreadable cache for explicit recovery; never dump Config |
| `src/vault_client.rs:108,194,260,286,352,442,719,789` | `VaultAuth` custom Debug already redacts token. Dynamic response/cache structs still contain raw strings. Credential process is shell command; inherits stdin/stderr/environment. HTTP token header not explicitly sensitive. Provider can return Vault address | Keep good Debug behavior; capture/suppress child stderr in agent mode; bounded helper/protocol, fixed approved address, sensitive headers, no agent helper overrides, disable unintended redirects |
| `src/pgpass.rs:113,172,214,261`; `src/myconf.rs:137,184,217,259,281` | **Confirmed:** legacy plaintext lookup and save. PG writer has owner-only creation mitigation; MySQL writes before chmod. Alternate `PGPASSFILE` / MySQL config sources and broad matching remain ambient authority | Read/import only for human legacy mode; do not export to these files as normal save; audit locations/permissions, never rewrite third-party files without explicit migration choice |
| `src/ai/key_storage.rs:46,117,141,173,190,197,229`; `src/ai/chatgpt_auth.rs:374,470` | **Confirmed:** env/keychain/machine-encrypted `ai_keys.enc`; file reader accepts plaintext; OAuth named secret automatically falls back to weak file on keyring failure; reads `~/.codex/auth.json` | Unified secure storage covers API keys and refresh/access tokens too; fail closed on locked keychain; no weak automatic downgrade; agent broker excludes AI auth imports |
| `Cargo.toml` keyring entry; keyring 3.6.3 `src/lib.rs:79,207` | **Correction:** `linux-native` is Linux **keyutils**, not Secret Service despite local comment; not the proposed persistent desktop keystore | Explicit reviewed Secret Service feature/backend; test headless behavior and persistence; report backend accurately |
| `src/config.rs:223,384,2646,2699`; documented save of `ssh_tunnel_patterns` | SavedSession has no primary password field, but serializes `ssh_password` and arbitrary options. SSH target strings can embed passwords; `password_command` may contain literal tokens | Versioned secret refs for SSH/option credentials; separate helper IDs from executable command text; migrate only fields proven secret without losing TLS options |
| `src/config.rs:246,2926,3009`; `src/cli_core.rs:1501,1513,3248`; `gui/core/src/connect.rs:343,497` | Normal CLI/GUI calls redact userinfo before recents, but storage API persists caller URL unchanged. Redaction does not remove secret query values/fragments; arbitrary options remain | Enforce secret-free typed records at persistence boundary, not caller discipline; scan existing recents/session options |
| `src/config.rs:1234,1276,1294,1312,1766,3329` | Most current TOML writes atomic/private initially; legacy writes plain `fs::write`; atomic writer follows symlinks and restores existing permission bits, including overly broad modes. TOML parse errors can quote offending lines | Dedicated hardened secret/policy writer; owner/mode/symlink checks where security requires; sanitize parse diagnostics to file/line/code, not source snippets |
| `src/config.rs:77,93`; `src/config.rs:2755,2778,2786` | **Confirmed:** password command is `sh -c`/`cmd /C`; first stderr line enters error. SSH backtick substitution puts command text and stderr in errors | Treat credential helpers as trusted code; never accept them from agent input; sanitized bounded errors and fixed argv; disable substitution in protected aliases |
| `src/cli_core.rs:862`; `src/database_mongodb.rs:233,239,292` | **Confirmed direct credential log sinks:** password-management logs `original_url`; Mongo logs constructed `username:password@...` URI | Highest-priority patch; never log resolved URLs; safe structured descriptors only |
| `src/cli.rs:366,384`; `src/cli_core.rs:572`; `src/password_sanitizer.rs:5,35,115` | Args custom Debug redacts URL userinfo/SSH password (good), but logs `-c` SQL/config subcommand content. URL sanitizer preserves query keys/values and comments; regex sanitizer limited and not globally applied | Safe Debug for all request/config types; no SQL/option payload logging; central allowlisted output DTOs; regex only secondary defense |
| `src/db.rs:1671`; `src/database_clickhouse.rs:438,483`; `src/database_elasticsearch.rs:992,1057`; `src/database_mongodb.rs:540,1483`; `src/database_postgresql.rs:1095,1585,1601`; `src/database_datafusion.rs:664` | **Confirmed:** queries, filters, response bodies, EXPLAIN literals and decoded values enter debug logs. They may contain user data/secrets independent of connection URL | Remove raw values/query bodies from default debug; protected mode never logs these; fingerprint + size + timings |
| `src/main.rs:14,42,134`; `src/logging.rs:31,58,109,127`; `src/cli_core.rs:410,919,1065,1675` | Tracing and legacy debug.log are separate sinks; console tracing may use stdout; `{e}` messages pass through DB/helper/HTTP error text | Unify safe logging; MCP stdout reserved for protocol; public sanitized error code/message, private diagnostics still no secrets; private file creation and bounded retention |
| `src/commands.rs:1357,1380,1576,1609,2543`; `src/config_editor.rs:540,1195,1207,1816` | `\config show` intentionally whitelist-renders, SSH display masked; sessions/listpass do not show primary passwords (good). Config getter for `vault_credential_process` returns entire command; recent display_name is caller data | Preserve whitelisted views, mark secret-capable settings and forbid helper text disclosure in agent views. Sanitize metadata loaded from old storage too |
| Requested `\conninfo` | **No `\conninfo` command found** in current command enum/parser/help by repository-wide search. Closest public connection-info APIs below | Any future command must use public alias/display DTO, never Debug dump |
| `src/lib.rs:297,335,348,430,459,473,512,619` | `PyDatabase` accepts a password; connection_info returns db/host/user, not password. `PyConfig` exposes limit/display/save only, no secret getter. **PyConnection.connection_url returns supplied raw URL**, stored as String. Errors interpolate backend errors | Redact/deprecate getter; alias connect API routed through broker; audit all Python exception conversions. Python process cannot be credential-isolated if it loads local secret-resolving runtime |
| `src/database.rs:702,708,1074,1245`; `src/db.rs:1420`; `src/config.rs:436` | Raw credentials cloneable in public Rust structs/getters and reconstructed URLs. `ConnectionInfo::to_url()` itself deliberately excludes password (good), but arbitrary consumers can inspect password field | Public descriptor separate from ephemeral private auth input; raw session reconstruction not exported to agent clients |
| `gui/core/src/types.rs:9,25`; `gui/core/src/url_builder.rs:67,321,443`; `gui/core/src/connect.rs:96,343`; `gui/core/src/worker.rs:856`; `gui/core/src/store.rs:751` | GUI has useful masking/secret-option filtering and metadata-only Vault summary. Requests/ConnectionSpec still Debug+String credentials; worker retains credentials for database switching; connection errors raw | Backend owns secrets, GUI sends secret refs after enrollment; custom Debug, metadata-only worker messages, no target-changing reconnect for protected aliases |
| `src/history_manager.rs:139,163,174`; REPL `src/cli_core.rs:1962,2018`; `src/config.rs:741`; `gui/core/src/store.rs:493,534`; `gui/core/src/history.rs:1` | CLI FileBackedHistory persists input; named queries persist full text; current GUI QueryHistory is in-memory, not evidence of an additional history file | Do not store broker queries in REPL history; scrub/suppress credential-management commands at input boundary; doctor scans named SQL/history; document query data retention |
| `src/script.rs:36,42,52,91`; `src/pager.rs:21,33`; `src/config_editor.rs:1848`; GUI `gui/src/clipboard.rs:39`, `gui/src/home/details.rs:500` | Editors/temp scripts and explicit exports contain SQL; pager receives result data over stdin; external commands inherit env. GUI Copy target is intentionally password-free; general result/SQL clipboard copies are raw | Broker has no editor/pager/clipboard/export-to-path tools; private temporary files; sanitize target copy; user-chosen exports remain an explicit data disclosure |
| `src/ssh_tunnel.rs:99,118,148` | SSH subprocess arguments carry target/key path, not DB password; `PasswordAuthentication=no` and BatchMode. Stored ssh_password is not proof of password forwarding | Remove misleading unused-password configuration or implement secure owner-only enrollment; freeze trusted jump host/key; no password in argv/env/askpass shell |
| `src/docker.rs:434,638,1013`; `src/docker_tunnel.rs` | Docker inspect imports container environment credentials; DockerTarget derived Debug includes password. Relay transmits DB protocol to container; Docker daemon access is already high privilege | Debug redaction; no ambient Docker discovery/daemon overrides for protected production aliases; existing daemon pinning is useful but not a security boundary from Docker-admin agent |
| `src/ai/agent.rs:162,285,309,427`; `src/lib.rs:980` | Internal AI tool loop intentionally sends schema/query/results to provider; progress output can contain query text; external Django context may contain connection strings | Disable internal AI in broker; same authorization API if later enabled, explicit data egress policy, scrub progress and provider errors |
| argv/environment, shell history, crashes/backups | Raw URL arguments exposed before program can sanitize; inherited `VAULT_TOKEN`/AI keys survive child spawning; core dumps/swap/copies/backups can retain plaintext | Alias-only tools, environment allowlist and no credential argv, no raw exports; best-effort zeroization/dump restriction; rotate exposed credentials, do not promise secure erase on APFS/SSD/snapshots |

Additional transport constraints: credentials must be tied to intended TLS server identity, not only an IP/port. Require certificate/hostname verification for protected network aliases, no insecure TLS flags, no uncontrolled HTTP redirects/proxy env overrides, no agent-provided CA/key paths. PostgreSQL `sslmode=require` alone is not equivalent to `verify-full`. Pin SSH host keys rather than trusting a new jump host on first use in production. Query languages can themselves reach files/network or set credentials; policy must address engine-specific functions and external tables, not only connection parsing.

## D. Threat model and recommended secure connection contract

### Attacker and protected assets

Assets: database/SSH passwords, Vault tokens and leases, API/OAuth/cloud credentials, TLS private keys, target integrity, policy/approval integrity, query data, audit evidence. Attacker may be a prompt-injected agent, malicious MCP client, untrusted plugin, or local process acting as the same user. Production role privilege abuse and secret extraction are separate problems.

An arbitrary same-user shell can read user-readable files and source, reproduce machine-derived keys, inspect accessible environment (`ps -E` on macOS, `/proc` where allowed), invoke `security`/`secret-tool` or the trusted DBCrust binary, replace user-owned executables/configuration, and debug processes where permitted. Linux ptrace/Yama settings and macOS hardened-runtime restrictions vary; do not assume every same-user process can always attach, or that denial of ptrace alone solves file/keychain authority. A PTY can be allocated by an agent. **TTY + inquire confirmation is a user-interface safeguard, not proof a human approved.** Root/admin, kernel compromise and a compromised broker are outside the proposed guarantee.

The agent also legitimately receives query results, which may contain secrets/PII. A row limit or SELECT-only parser does not prevent repeated extraction. Least-privilege DB roles/views/RLS, allowed data surfaces, rate/byte budgets and human review are necessary. A production admin credential hidden behind arbitrary SQL is not a safe agent connection.

### Secure storage and migration

Define `SecretStore { get, put, delete, status }` with typed `Unavailable`, `Locked`, `Denied`, `NotFound`, `Corrupt` failures. Use `secrecy`/`zeroize` (subject to normal dependency review); no Serialize/Display and custom redacted Debug. Credentials are exposed only inside the connector authentication call; driver libraries may copy them, so zeroization cannot promise removal of every heap copy.

Preferred storage: OS keychain per credential, opaque stable ID (`dbcrust/connection/<uuid>/<field>`), not host/user strings as the identity. Settings/session/recent files store only refs and nonsecret metadata. Encrypted fallback: versioned AEAD envelope with random salt/nonce, AAD covering version/secret ID/target revision; master key either a random 256-bit key protected by an available keystore or derived from an interactively entered passphrase using Argon2id with reviewed memory/time bounds. No machine identifiers as keys; no saved passphrase, argv/env passphrase, or “key stored next to ciphertext.” If keystore is absent and no user unlocked passphrase backend exists, persist nothing and return an actionable error. A locked keyring must not silently trigger another backend. Random-key-in-keychain encrypted file is useful for batching, not an escape from absent keychain.

Migration is a resumable two-phase operation:

1. Inventory old `.dbcrust` encrypted/plaintext entries, SSH passwords/options, legacy config session data, recent URL credentials, AI keys/OAuth fallback and Vault caches without printing values. Parse the real escaped `.dbcrust` format, not a naive colon split. Do not invoke helpers while scanning.
2. Stage secrets under new IDs, read back/verify, and atomically persist secret refs plus schema version/migration journal. Lock related stores against competing CLI/GUI writers; journal contains IDs/state only. Crash after either stage must be safe to retry; garbage-collect orphan secret IDs after successful completion, not before.
3. Only after verification remove migrated DBCrust-owned plaintext/weak fields. Retain necessary rollback material encrypted under the new key, not a plaintext `.bak`. Unreadable legacy entries are reported and retained, never dropped. Older binaries must detect upgraded schema and refuse destructive writes. Do not silently migrate broad wildcard entries into protected aliases; owner confirms exact target bindings.
4. `.pgpass`, `.my.cnf`, shell history, `.vault-token`, external OAuth files and backups are not automatically deleted; doctor reports location/risk, owner selects remediation. Raw credential URLs should not be “migrated” from arbitrary query text automatically. Recommend rotation when prior logs/history exposure is plausible; deletion cannot revoke copies.

`dbcrust doctor --secrets [--format json]` is read-only by default; results contain rule ID, path, location, severity and remediation, never matching secret content. Detect credential-bearing userinfo, query params/tokens, SSH password syntax, literal helper credentials, plaintext `.dbcrust`/AI files, unsafe permissions, insecure TLS, unmigrated refs and unavailable providers. Scan only known configured locations with byte/file limits; don't invoke shells, decrypt for display or recursively follow arbitrary symlinks. Optional explicit `--fix`/migration subcommand is a separate operation with `inquire` for interactive choices. Test with synthetic canaries and injected paths/providers, never real keychain or home.

### Platform assurance

| Platform | What can be relied on | Limits and implementation decision |
|---|---|---|
| macOS | Keychain protects stored values; access decisions can be tied to an application/code requirement and user interaction | Current keyring 3.6.3 `macos.rs:56` delegates generic-password storage through security-framework; no DBCrust-specific biometric/access-control policy is configured. Default legacy Keychain ACL behavior, signed designated requirements, prompts and inherited existing item ACLs matter. Stable signed broker identity is preferable; cargo/mise/unsigned/ad-hoc binaries may change identity on rebuild or have unreliable trust continuity. A same-user agent can invoke an authorized binary. `security` access may prompt/deny depending on ACL; neither “always readable” nor “always safe” is justified. Do not claim biometric/per-use guarantees from `keyring` alone |
| Linux desktop | Secret Service collection encryption/unlock via GNOME Keyring/KWallet | Existing feature is keyutils, not Secret Service. Session bus availability and unlock vary; same-user unlocked bus clients are not meaningfully isolated by the generic API. Add explicitly tested persistent backend with bounded blocking operations |
| Headless Linux | Passphrase-unlocked encrypted backend or broker service with externally provisioned short-lived credentials | No assumed login keyring/DBus/desktop prompts. Service startup should fail locked without provisioning. Separate UID, filesystem ACLs, socket permissions, ptrace restrictions and network policy matter more than cosmetic storage backend |
| Windows | Credential Manager stores secrets for the principal; service-account isolation and named-pipe ACLs are available | Generic credentials are not per-application isolation from the same user; use broker service identity, strict pipe client validation and Windows ACLs. No Unix permission emulation claims |

Before advertising macOS protection, a dedicated Phase 1 spike must create **synthetic items in a temporary test keychain**, test same binary, another binary, `security`, signed upgrade with same designated requirement, changed signer, unsigned/ad-hoc rebuild, moved path, locked keychain, no UI session, and cancellation. Record OS/signing versions. It must not modify the user's login keychain. Source review establishes lack of explicit ACL configuration; ACL prompts/upgrade behavior were **not empirically measured in this round**. The generic keyring crate cannot by itself supply a cross-platform hostile-same-user guarantee.

### Agent-safe alias and broker API

Keep `session://prod` for familiarity. Add an owner-enrolled protected record with UUID, fixed engine/host/port/database/user, TLS identity and trust roots, authorized tunnel/proxy chain, credential refs, policy revision and agent exposure mode. Existing sessions default to **agent deny**, not grandfathered production access. Agent schema accepts `connection_alias`, query/parameters and tighter optional limits, never URL/credential/helper/host/path overrides. Reject alias query strings/fragments/userinfo, `session://prod?host=evil`, nested `recent://`, `--ssh-tunnel`, database switching, Vault address overrides and environment target overrides. Resolve aliases from broker-owned state; if metadata and protected descriptor disagree, fail closed.

The protected descriptor must not be just an agent-writable TOML row pointing at a keychain secret: changing its host would exfiltrate the password. In isolated mode the broker owns configuration and secret refs. Local convenience mode can bind authenticated descriptor/AAD/revision in secure storage and require owner re-enrollment to change it, but cannot claim tamper resistance against the same user who can exercise the same credential APIs. Do not reuse wildcard passwords for protected records.

Broker flow:

```text
Human enrolls alias + narrow DB role + policy in trusted owner UI
    → secure store / authoritative descriptor
Agent MCP client → credential-free stdio adapter → authenticated broker query endpoint
    → authenticate capability → resolve immutable alias → classify → policy/budgets
    → append audit start → resolve credential internally → execute with native restrictions
    → bounded sanitized result + completion audit
Write request → pending approval → separate trusted human UI → exact one-use execution
```

Use `dbcrust mcp` as the familiar stdio forwarding entry point; it must not silently start/unlock a secret-bearing broker in the agent's context. Also ship a small `dbcrust-agent` binary from agent-client for isolated installs so the agent-facing executable has **no keystore or drivers linked**. The trusted owner starts `dbcrust broker`/`dbcrust-broker` outside the agent runtime. Unix socket in private directory and Windows named pipe with explicit ACL; authenticate peer OS identity (`getpeereid`/`SO_PEERCRED` or platform equivalent) plus scoped session capability. UID is not sufficient to distinguish same-user clients.

Capabilities: revocable, short-lived, random, bound to client principal, permitted aliases, maximum policy, expiry and broker generation. They authorize queries and are secrets in their own right, but are not database credentials. Deliver over an inherited pipe/FD or isolated credential channel, not argv/env/shared global token. If a user-readable capability file is offered for convenience, label the same-user theft limit explicitly. In isolated deployments client bootstrap can use approved peer identity to mint only its preassigned query scope; never mint owner/admin capability. Client-id text cannot select a more privileged identity. No listening TCP endpoint by default.

Human admin/approval uses a separate endpoint inaccessible to the agent. An owner terminal may use `inquire::Confirm` with query, alias, effect class and bounds visible; GUI may render the pending request and notify the owner, but a notification alone is not authorization. An agent-provided TTY, `--yes`, forged client label or MCP “approve” tool cannot grant approval. If no owner approval channel is available, return `approval_required`; never hang MCP stdin waiting for an interactive prompt. GUI controls stay in the GUI; any CLI prompt uses inquire, per project rules.

Each approval binds client principal, alias UUID, target/policy revision, tool, canonical argument bytes/parameters, budgets and expiry; consume transactionally once. Editing named scripts/queries after review invalidates approval by content digest. Crash after commit but before response produces **unknown outcome**, not automatic retry. Exactly-once database mutation cannot be guaranteed across arbitrary engines; require idempotency keys/domain checks where available and make at-most-once broker dispatch explicit.

### Policy and execution enforcement

Per connection: `deny`, `read_only`, `read_write`; default deny for agent exposure. Read-write defaults to Ask for writes, DDL/admin denied unless separately enrolled. Effective policy = intersection of client grants and immutable connection ceiling; CLI flags may tighten but cannot disable it (`--read-only=false`, timeout=0, max_rows=0 cannot escape). Suggested defaults: 500 returned rows, 30 s execution, 8 MiB response, bounded cell/frame size, 1 active query per alias/client and bounded queue/rate. These are initial product defaults, not universal workload limits. Record truncation explicitly.

Start with metadata, bounded parameterized query, explain-without-analyze, cancel and approval-status MCP tools. No `get_password`, raw-config, shell, file export, credential command, arbitrary URI, internal AI invocation or approval tools. Server enumerates only authorized aliases/capabilities; tool schema descriptions and logs never contain host credentials. Sensitive connection metadata can be alias-only; returning a hostname is a separate owner option.

Use engine-aware parsers/structured operations with default-deny unknowns. Existing `safety.rs` documents its textual limitations; it is a helpful fallback, not the policy boundary. PostgreSQL dedicated read-only role, restricted schema/search_path/function privileges, transaction read-only and server statement/lock timeouts; no superuser, file/program/network extension privileges. MySQL needs role grants + native read-only transaction where supported; current implementation lacks that hardening. SQLite use read-only open mode plus query-only/authorizer and disallow ATTACH/extensions/unapproved paths. Redis restrict ACL command/key patterns and ban eval/module/admin/blocking commands initially. Mongo restrict DB role and disallow write aggregation stages. Cloud providers enforce IAM scope; S3 prefix/bucket ARN binding; CloudWatch cost and time limits. DataFusion/DuckDB must be denied for production aliases until file/network/UDF/extension escape surfaces are constrained.

Schema/table allowlists over arbitrary SQL are not a simple string filter: views, functions, search_path, joins, CTEs and dynamic SQL undermine it. Prefer DB grants/RLS and approved query templates; enforce optional broker table allowlists only for structured operations with resolved identifiers. Do not promise arbitrary SQL table security from an AST allowlist alone.

Limits are enforced **while fetching**, not just by appending LIMIT or truncating output. Include metadata, explain and additional result sets. Apply server timeout where supported, client deadline, cancellation and pool discard if state is unknown; report that client cancellation does not always stop server work. Driver must reject required protections it cannot implement. Separate read/write pools and validate reconnect/reset state; no shared global mutable read-only flags.

Audit every attempt, including denied, pending, cancelled and failed queries. Record timestamp, broker/client identity, alias UUID/revision, operation class, decision, approval/request IDs, query fingerprint/length, duration, returned rows/bytes, truncation and sanitized outcome. No passwords, parameters, raw query text/results by default. Use keyed fingerprints if low-entropy query guessing is a concern. Fail before production execution if mandatory audit start cannot be persisted; append outcomes after execution, and preserve unknown-outcome recovery records. Local JSONL with a single bounded writer/rotation is sufficient initially; approval state needs atomic durable consumption. A SQLite journal is reasonable later, but share SQLx's existing SQLite library rather than introduce conflicting rusqlite `links=sqlite3`. Neither format is tamper-proof to its owner; export to an external append-only sink for stronger audit assurance. Hash chaining without an external anchor does not stop deletion or full rewrite.

### Guarantees and non-guarantees

**Guarantees targeted and tested:** DBCrust-owned new credential stores have no plaintext credentials; no silent weak fallback; agent-facing protocol and documented commands never export stored connection credentials; protected aliases cannot redirect credentials via request overrides; policies/approvals/limits apply inside broker to all access paths; bounded results and correlated audit events; no raw database credential in agent-client memory, argv or environment when using the broker architecture. Normal DB driver/broker memory necessarily contains authentication material.

**Conditional guarantee:** a malicious agent cannot read stored credentials or rewrite policy only when broker/store/admin channels are outside its OS authority and sandbox/filesystem/network/debug restrictions are correctly deployed. Tokens stolen from the agent grant at most its enrolled scope, not raw passwords or admin access.

**Non-guarantees:** arbitrary same-user shell isolation, protection from root/admin or compromised trusted broker/driver, perfect zeroization, secure erasure of old copies, harmless arbitrary SELECT/UDF, prevention of allowed data exfiltration, tamper-proof local audit, guaranteed server-side cancellation for every engine, or exactly-once cross-engine writes after crashes. A local keychain plus TTY confirmation cannot supply these guarantees.

### Agent guide and MCP client configuration

Update `src/agent_guide.md` (currently describes shell-only use and loosely calls legacy storage a keystore) to explain enrollment, alias discovery, policy errors, pending approvals, safe retry semantics and the two assurance levels. Keep existing one-shot JSON/exit-code compatibility; expose structured new error codes without changing old exit meanings unnecessarily. Never recommend raw password URLs/env variables as the agent setup path.

The following are **proposed future DBCrust commands**, not commands implemented today. Connection secrets are absent from configurations; `--client codex` is a label and never the authentication mechanism. Prefer absolute installed binary paths to PATH lookup in broker deployments.

Codex `~/.codex/config.toml` example, schema checked against [official MCP documentation](https://developers.openai.com/codex/mcp/):

```toml
[mcp_servers.dbcrust]
command = "/absolute/path/to/dbcrust-agent"
args = ["mcp", "--client", "codex"]
```

Equivalent registration shape: `codex mcp add dbcrust -- /absolute/path/to/dbcrust-agent mcp --client codex`. No DB credentials or bearer capability in `env`; the installed client's approved local bootstrap determines its scope. Omission of config env fields does not by itself clear inherited environment: agent-client and broker must enforce their own allowlist.

Cursor user/project `mcp.json` example ([official docs inspected](https://cursor.com/docs/context/mcp)) and proposed Claude Code `.mcp.json` shape:

```json
{"mcpServers":{"dbcrust":{"command":"/absolute/path/to/dbcrust-agent","args":["mcp","--client","coding-agent"]}}}
```

Claude Code's official MCP page returned HTTP 403 during this research; revalidate exact file scope/registration command against its installed version when shipping docs. The JSON is the intended standard stdio configuration, not a tested end-to-end client integration. Client-side approvals/tool filters are useful defense-in-depth but never replace broker enforcement. No MCP configs were installed in this round.

## E. Ordered PR-sized implementation roadmap

Sizes are engineering estimates including review/tests, not deadlines: S≈1–2 days, M≈3–5 days, L≈1–2 weeks for one experienced contributor. A large source move may have a large diff but small behavioral scope. Each row is independently reviewable; do not combine a dependency major upgrade with an extraction.

| PR | Scope and main paths | Tests and validation | Docs | Size / risk |
|---|---|---|---|---|
| **1a** | Close confirmed credential/query log sinks; safe Debug/error DTOs; sanitizer/persistence boundaries. `cli_core.rs`, `database_mongodb.rs`, other driver log sites, `password_sanitizer.rs`, `config.rs`, `lib.rs`, GUI types/errors | Inline rstest canary matrix for userinfo/options/encoded/malformed URLs, stderr, Debug, Python getter; synthetic fake servers/helpers only | security/storage guide; agent guide; CLAUDE rules | M / medium compatibility |
| **1b** | `SecretStore` module, native providers, explicit locked/unavailable states; replace AI fallback; secure atomic writer; platform spike | Fake providers; no-keyring/locked/error/retry/concurrent tests; temporary macOS test keychain/Windows ACL/Linux Secret Service matrix outside ordinary unit suite | setup by platform, fallback/unlock/recovery | L / high data safety |
| **1c** | Versioned secret refs, resumable `.dbcrust`/SSH/AI/session/recent migration; Vault cache handling; update FieldSpec/save docs for new settings | Crash points, interrupted readback, duplicate/wildcard entries, escaped fields, malformed data, old-schema compatibility, concurrent CLI/GUI writer tests | migration/rollback/rotation guide | L / high |
| **1d** | `doctor --secrets` and remediation UX, enum-based command registration. `cli.rs`, `commands.rs`, `config_editor.rs`, guide | Inline rstest fake file corpus; no values in report/errors; bounded scan, symlink/permission checks; doctor never executes helper | `docs/src/content/docs/reference/backslash-commands.md`, new `user-guide/secure-connections.md` | M / medium |
| **2a** | Protected alias descriptor, target binding, exposure deny default, policy types; explicit per-connection read-only/options replacing globals. `config.rs`, `database.rs`, `db.rs`, CLI/GUI connect | Alias overrides, env override, stale descriptor, direct URL access, cross-session flag races, disabled protections; native PG gate via DATABASE_URL | protected session enrollment, data access limits | L / high |
| **2b** | Shared execution gateway, streaming budgets, structured operation classifier, audit writer; PG native safeguards first. `safety.rs`, `db.rs`, driver APIs, GUI worker, AI tool adapter | Textual bypass corpus, CTE/UDF/multi-statement cases, native PG role restrictions, cancellation/reconnect, byte/row/time budgets, audit fail-closed. Do not assert mock proves DB semantics | policy/audit reference and operator guide | L / high |
| **2c** | Broker/protocol/credential-free client + small MCP tool set, version/auth handshake, revoked tokens; process separation and no ambient helper authority | Process tests in inline modules spawning test fixture children; forged identities, replay, path/host changes, env canaries, no secrets in client outputs; MCP protocol conformance | `user-guide/agents.md`, MCP client examples, `src/agent_guide.md` | L / high |
| **2d** | Durable one-use write approvals, separate owner endpoint, CLI inquire and GUI pending view; policy reload/revocation | Replay, expiry, changed args/script/target/policy, denied self-approval, concurrent consume, crash-before/after dispatch unknown outcome | human approval and safe retry guide | L / high |
| **2e** | Production isolation packaging/reference deployment, signed broker distribution, systemd/service/pipe/socket ACLs, limited-network agent example | Separate-principal hostile shell test: cannot read store, connect admin endpoint, alter policy, debug broker, fetch unapproved alias; read-only role is tested at DB | deployment threat model, exact guarantees/non-guarantees | L / high; release gate for strong claim |
| **3a** | Extract model/driver-api and pure SQL/format helpers, compatibility reexports. Move TableDetails out of db.rs before traits | Existing inline tests; facade API compilation; no engine deps in model/API `cargo tree` | workspace dependency map + CLAUDE module map | M–L / medium |
| **3b** | Extract secret/storage/policy/audit/connect modules behind already-stable APIs; no new storage migration in this PR | Existing migration/contract suites, schema/save documentation sync, isolated GUI-core tests | contributor docs | M–L / medium |
| **3c** | Extract **one existing driver per PR**: SQLite, PostgreSQL, MySQL, White Dragon, ClickHouse, MongoDB, Elasticsearch, files. Add optional deps and registry forwarding; keep bundled defaults | Single-driver/no-default/bundled feature builds; inline driver tests; PG gate; snapshots/output regression; disabled engine typed errors | connector features, build recipes | S–M each, files/PG M–L / medium |
| **3d** | Extract runtime then CLI/optional AI; GUI consumes lower crates; frontend adapters preserve behavior; explicit connection settings replace remaining globals | CLI one-shot/commands, GUI workers, Python build + native smoke; forbidden dependency graph checks | architecture, Python and GUI developer docs | L / high cycle/API risk |
| **3e** | Feature presets, packaging, targeted profile/TLS quick wins, release-size budgets; separate extension packaging only if approved | Repeated cache-controlled timing/size matrix; i686 Python compatibility; feature combinations; shipped binary/wheel smoke | installation matrix and release notes | M / medium |
| **4a** | Redis driver crate, SCAN/typed bounded reads, TLS/ACL and capabilities; new enum schemes/commands via strum | Inline protocol/fake tests; optional container harness documented separately; deny eval/admin/native command bypass | `reference/redis.md`, examples | M–L / medium |
| **4b** | SQL Server driver crate, metadata, SQL auth/TLS, parameterized execution and cancellation | TDS integration environment plus inline types/limits/error tests; no unsupported read-only claim | `reference/sql-server.md` | L / high driver semantics |
| **4c** | Redshift crate using shared PG transport; read-only catalogs and type compatibility | Redshift test account integration; metadata snapshots; refuse unsupported writes/PG-only commands | `reference/redshift.md` | M / medium |
| **5a** | InfluxDB v1/v2; explicit query language and annotated CSV streaming | Mock HTTP fixtures: tokens absent errors/logs, Flux/InfluxQL, compression/limits/cancel | engine page | M / medium |
| **5b** | Remote libSQL/Turso spike then connector; avoid bundled SQLite collision | Manifest/cooldown review, remote auth/token redaction, statement/transaction capability tests | engine page | M–L / medium |
| **5c** | Shared optional AWS auth module, then DynamoDB, CloudWatch, S3 **separate PRs/crates** | IAM role/endpoint binding; SDK errors, pagination, bounded polling/scan, CloudWatch stop and cost window, object prefix policy | cloud auth + one page per engine | L auth, M–L each / high auth/cost |
| **6a** | DuckDB genuine adapter opt-in; separate DuckLake attachment/extension PR | File lock/read-only, extension/network deny, memory/row limits; cross-platform native build, catalog secrets | DuckDB then DuckLake guides | L each / high build/security |
| **6b** | External RPC driver protocol/host and SDK example, trusted enrollment only | Compatibility versions, oversized/malformed frame, dead worker/cancel, lying capability refusal, secret-free transport diagnostics | driver authoring/protocol/trust guide | L / high |

Every feature PR follows project rules: inline `#[cfg(test)]`, `rstest` parameterization, injectable paths/fake secret provider, isolated `DBCRUST_CONFIG_DIR=$(mktemp -d)` and `RUST_TEST_MODE=1`, no `cargo run` as tests. Do not assume config isolation redirects `.pgpass`, `.dbcrust`, Vault or native keychains: inject those paths or use fixture child processes and no real credentials. New backends get explicit optional integration harnesses; current repository only gates PG on DATABASE_URL.

Build-green checks at each extraction: mise Cargo `--locked` unit tests, dev build, clippy `--all-targets -- -D warnings`; GUI-core tests plus GUI compilation; `--features python` compilation and maturin wheel import/one-shot smoke; pure-Python `mise run py:test`. Feature CI: zero connectors, each connector alone, standard, bundled, python+bundled, GUI-core+standard, GUI+bundled, all supported combinations on Linux/macOS/Windows, and required 32-bit Python check for dependency changes. Do not run all-feature GUI builds as the default developer loop. Reuse schema-sync tests (`test_schema_matches_serialized_config`, `test_documented_save_covers_schema`) whenever fields change; root TOML keys before tables. New public APIs need rustdoc; all new command/help lists come from strum.

### Parallel worktrees and dependency ordering

- Phase 1a log/redaction fixes and a **read-only macOS/backend storage spike** can run in separate worktrees. 1b storage contract must settle before 1c migration; 1d detector corpus can start independently and merge after migration schema is fixed.
- A pure model/API design/spike and cache/profile measurement can run alongside Phase 1. Do not perform a mass source move while storage/security patches are in flight; it creates avoidable conflict and hides behavioral review.
- 2a/2b establish the authority/execution contracts before 2c/2d. GUI approval UI can be built against protocol fixtures while broker code develops; strong deployment packaging follows working protocol and threat tests.
- After 3a/registry API, existing drivers can be extracted in separate worktrees **with one integrator owning root manifests/lockfile/facade exports**. Move dependent pure helpers first. Driver crate code should not independently reformat the root tree.
- New Redis, MSSQL and Redshift worktrees can run in parallel once 3c/API/policy conformance is stable. AWS auth lands before three cloud drivers. DuckLake depends on a real DuckDB connector. External RPC is independent of cloud/embedded engine work but depends on stable capabilities and broker trust rules.
- Each worktree starts from a common green committed integration base in implementation rounds. This planning round does not create commits, branches, or delegated workers.

## F. Owner decisions (recommendations)

1. **What does “agent cannot see the password” mean?** Recommend both tiers, with isolated broker deployment required before claiming resistance to a malicious same-user shell. Local keychain mode remains valuable but must say what it protects.
2. **First production engine and role?** PostgreSQL only for the first protected release, with a dedicated narrowly scoped role/replica. Do not reuse the owner's superuser credential. Extend enforced modes engine by engine after conformance tests.
3. **Write policy?** Read-only by default; explicit exposure is deny until enrolled; writes require separate human one-use approval; DDL/admin denied initially. No same-client approval API.
4. **Headless storage fallback?** Passphrase-unlocked encrypted file or service-provisioned short-lived credentials; fail closed if unavailable. Never automatic machine-key fallback. Confirm whether unattended broker startup is a requirement; if yes, specify service identity/provisioning rather than storing its passphrase.
5. **macOS distribution?** Recommend a stable signed broker identity for production distribution; cargo/mise installs remain development/local-convenience paths until upgrade/ACL behavior is tested. Generic keyring API alone does not promise per-use biometric authorization.
6. **Default build composition?** Preserve bundled defaults while moving code; later source default standard (PG/MySQL/SQLite + SSH/Vault), full prebuilt binaries/wheels explicitly bundled. New cloud/embedded connectors opt-in. Confirm whether full-size convenience or minimal default distribution matters most.
7. **Connector ordering?** Redis + SQL Server, Redshift; InfluxDB + remote libSQL; shared AWS then DynamoDB/CloudWatch/S3; DuckDB/DuckLake and external plugins last. S3 starts read/list/query integration rather than a full file manager.
8. **Audit retention/data?** Fingerprints and metadata only, proposed 30-day bounded local retention; separate opt-in secure full-query capture if an organization needs it. Strong audit integrity needs external collection. Decide retention and whether mandatory audit failure should block all protected queries (recommended yes).
9. **Secret migration scope and legacy compatibility?** Automatically migrate verified DBCrust-owned values with safe recovery; explicit choice for third-party `.pgpass`/`.my.cnf` removal. Deprecate raw Python connection_url getter in favor of redacted descriptor. Do not break ordinary Python execution or maturin ABI.
10. **External driver trust?** Signed/reviewed owner-enrolled workers only; disabled for production protected aliases until audited. Do not market arbitrary external drivers as credential-blind.

## Research limits and follow-up evidence

No tracked source contents changed, no commit, no real DB queries, no production credential inspection, and no user keychain experiment. dbflux was source/doc-reviewed, not built or security-certified. GitHub API tree discovery hit HTTP 403 rate limit; pinned raw source retrieval remained available. Connector costs are qualified forecasts until feature-isolated measurements exist. Native keychain prompt/upgrade behavior remains an explicit experimental gate. End-to-end MCP configs describe the proposed product, not existing commands.

Release/size measurement completed in round 2 (section B table). `cargo bloat --crates` was deferred to Wave 2a, where it runs before and after feature-gating.

## G. Round 2: owner decisions, review response and agreed roadmap

Round 2 was run by the Opus orchestrator on the Claude perso provider. The ChatGPT-perso provider failed with 401s, and the owner asked to continue on Claude perso only. **Implementation subtasks use Claude Sonnet 5.5.** This section **supersedes section E wherever they disagree.**

### G.1 Owner decisions (binding)

- **D1: isolated broker from day one.**
  - A credential-owning broker holds every agent-usable secret.
  - Agents talk to it through a thin client (`dbcrust-agent`, MCP stdio + one-shot CLI) that links **no keystore, no drivers, no config storage**.
  - **Service mode:** the broker runs as a separate OS identity (launchd daemon / systemd service under a dedicated user). This is the supported "prod" deployment.
  - **Same-user mode:** for local dev. Its guarantee is weaker and stated honestly.
- **D2: agent writes.**
  - Agent exposure is off until the owner enrolls a connection.
  - Enrolled connections are read-only by default.
  - The owner may allow DML per connection at enrollment. DDL/DCL/admin is always refused.
  - No per-write approval flow in v1.
- **D3: default build** keeps every engine shipped today. Heavy new connectors (DuckDB, AWS) are opt-in. A slim PG/MySQL/SQLite preset exists.
- **D4: all connector groups, in waves.**
  1. Redis + SQL Server
  2. Redshift + libSQL/Turso
  3. AWS (S3, DynamoDB, CloudWatch; InfluxDB rides along) and DuckDB (+DuckLake)

### G.2 Review outcomes (round-2 objections)

- **O1, scope.** The roadmap in E is too long before the killer feature ships. Wave 1 is cut to five sequential tasks (G.4). It includes D1's broker and secure storage, and nothing else. These move to Wave 4:
  - durable one-use approvals;
  - GUI approval/pending UI;
  - signed distribution;
  - table allowlists;
  - streaming budgets beyond auto-LIMIT + fetch-side truncation;
  - Windows named-pipe broker.
- **O2, macOS keychain, verified at source.**
  - keyring 3.6.3 `src/macos.rs:57` calls `SecKeychain::set_generic_password`.
  - That reaches `security-framework 2.11.1 src/os/macos/passwords.rs:295`, i.e. `SecKeychainAddGenericPassword(..., ptr::null_mut())`.
  - So it uses the **legacy keychain with a NULL initial access**, i.e. the default ACL that trusts only the creating application.
  - Another binary (`security find-generic-password -w`, python, a replaced or rebuilt dbcrust) triggers a user prompt.
  - Cargo/mise ad-hoc builds change cdhash on every rebuild, so each upgrade prompts once ("Always Allow").
  - It is still not a boundary against a same-user agent *invoking the trusted dbcrust binary*. Hence D1: protected secrets live in the broker, and the regular CLI refuses broker-owned records (G.3).
  - **Empirical prompt behaviour is still unmeasured.** It is a release gate, tested with a throwaway keychain in `/tmp`, never the login keychain.
- **O3, tamper-proof binding.**
  - Every stored secret is a versioned payload `{v, secret, bound_target, agent_policy?, revision, created_at}`. `bound_target` holds engine, host, port, db, user and TLS mode.
  - dbcrust refuses to use a secret whose `bound_target` differs from the resolved target. This applies to human sessions too: redirecting `session://prod` to evil.example fails closed.
  - The policy ceiling is read **from the payload**, never from TOML.
  - Changing a target, or loosening policy, needs re-enrollment, which needs the secret re-entered: knowing the secret is the owner proof.
  - Tightening or revoking needs no proof; an agent could revoke, but that is DoS, not disclosure.
- **O4, crate count.**
  - Wave 1 adds only two new crates: `crates/agent-protocol` (serde DTOs, no I/O) and `crates/agent` (bin `dbcrust-agent`). The root becomes a workspace member list: `.`, `crates/*`, `gui`, `gui/core`. `default-members = [".", "crates/agent"]`, so `cargo build` also builds the agent binary.
  - The first real split (Wave 2) targets about 12 crates, split along heavy dependencies:
    - `dbcrust-core` (model + driver API + SQL helpers + format + policy types)
    - `dbcrust-secrets`
    - `dbcrust-driver-{postgres,mysql,sqlite,mongodb,clickhouse,elasticsearch,white-dragon,files}`
    - `dbcrust-ai`
    - `dbcrust-docker`
    - `dbcrust-broker`
    - the root (CLI + PyO3 facade)
  - Quick wins (Wave 2a, before the split):
    - optional features for files/ai/mongodb/elasticsearch/docker/explain-tui/clipboard (default = all, D3);
    - genai/reqwest-0.13 TLS provider unification, to stop compiling aws-lc-sys next to ring;
    - `debug = "line-tables-only"` for dev deps.
- **O5, agent surfaces.**
  - `dbcrust-agent mcp`, a stdio MCP server with tools `list_connections`, `list_tables`, `describe_table`, `query`, `explain`.
  - `dbcrust-agent list|tables|describe|query|explain`, one-shot with `-o json|jsonl|csv|table`, mirroring today's JSON envelope and exit codes.
  - **MCP is hand-rolled**: JSON-RPC 2.0 over newline-delimited stdio (`initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call`), about 400 lines with serde_json. `rmcp` is rejected for v1: schemars/macros transitive weight, and the protocol surface needed is tiny.
  - **IPC** is `tokio::net::UnixListener` with `peer_cred()` (uid/gid/pid). Windows broker is Wave 4. On Windows the agent binary builds and reports `unsupported_platform`.
  - Direct `dbcrust session://alias` stays for humans. It refuses broker-owned records (they are not in the human store at all in service mode).
- **O6, storage.**
  - **macOS/Windows:** OS keychain first.
  - **Linux:** Argon2id passphrase-encrypted file (passphrase via `inquire::Password`, never argv/env/disk). The derived master key is cached in the kernel keyutils session keyring (already compiled via `linux-native`) with a timeout, so it is unlocked once per login session. Secret Service is an opt-in later feature: it adds dbus/zbus to manylinux wheel builds.
  - No machine-derived keys ever. The machine-derived scheme stays only as a read-only migration reader.
  - The same store serves AI keys and OAuth tokens. The automatic weak-file fallback is removed (fail closed).
  - **Broker service mode** uses its own encrypted file store in a 0700 state dir owned by the service user, with `key_source = "file" | "passphrase"`:
    - `file`: a 0600 random key owned by the service user. The OS permission is the boundary; encryption covers backups.
    - `passphrase`: the owner runs `sudo dbcrust broker unlock` after start; the key is held in memory only.
    - systemd `LoadCredentialEncrypted` / TPM is Wave 4.
- **O7, ordering accepted.** Wave 1 security (G.4), then Wave 2 build quick wins + split, then Wave 3 connectors in parallel worktrees, then Wave 4 approvals/GUI/Windows/signing. Nothing of the split needs to precede the broker, because the thin agent crate is new code.

### G.3 Guarantees: per platform and deployment mode

| Mode | Agent can read a protected password? | Agent can redirect it to another host? | Agent can loosen policy? | Agent can bypass policy? |
|---|---|---|---|---|
| Service mode (broker as separate OS user), macOS/Linux | **No**: store and key are owned by the service user (root/admin excluded) | **No**: binding + broker-owned state | **No**: admin ops need `sudo`; the admin socket accepts peer uid 0/service uid only | **No** for broker aliases. Only the owner's *own* human sessions remain reachable through the regular CLI; documented: don't keep prod admin creds in the agent's user account |
| Same-user mode, macOS | No silent read (keychain ACL prompts for any other binary); residuals: user clicks Allow, debugger attach with DevToolsSecurity enabled | **No** (payload binding) | **No** (re-enrollment needs the secret) | Weak: the agent can run the trusted binary; broker records are refused by the regular CLI, but a same-user agent can start its own `dbcrust broker` (same payload policy applies, so not a bypass of the ceiling) |
| Same-user mode, Linux/Windows | **Yes, possible**: no per-app ACL (keyutils/Credential Manager readable by the same user) | No (binding) unless it reads the secret | No without the secret | Convenience-grade only; documented as such |

Non-guarantees (all modes):
- root/admin;
- a compromised broker binary;
- data exfiltration through allowed SELECTs (use a least-privilege DB role);
- server-side cancellation on every engine;
- perfect zeroization;
- tamper-proof local audit.

### G.4 Wave 1: ordered implementation tasks (one branch, sequential commits)

General rules for every task:
- Follow CLAUDE.md: strum, inquire, `FieldSpec` + `save_with_documentation` for config fields, rstest inline tests, `mise exec -- cargo ... --locked`.
- Tests run with `DBCRUST_CONFIG_DIR=$(mktemp -d) RUST_TEST_MODE=1`. **Tests never touch the real keychain or $HOME**: inject a fake `SecretBackend`.
- New deps must satisfy the 24h cooldown + `check_cargo_cooldown.py`.
- `cargo clippy --all-targets -- -D warnings` stays clean. The GUI crates (`mise run gui:test`, `gui:lint`) and `--features python` must still compile.

**W1-T1: close credential leak sinks** (small)
- **Remove secret-bearing logs and Debug output:**
  - `src/cli_core.rs:862` (raw URL) and `src/database_mongodb.rs:236,292` (URI with password). Sweep every `debug!/info!/trace!/eprintln!` that formats a URL, ConnectionInfo, DSN or connection string in `src/` and `gui/core/src/`.
  - Replace derived `Debug` with a redacting manual impl on `ConnectionInfo` (`src/database.rs:702`), `DockerTarget`, `SSHTunnelConfig`, the Vault credential structs, `SavedSession` options (redact secret-ish keys), and the GUI `ConnectionSpec`/requests.
- **Central helper** in `password_sanitizer.rs`:
  - `redact_url(&str) -> String` strips userinfo password **and** values of secret-ish query keys (`password`, `pass`, `pwd`, `token`, `secret`, `api_key`, `apikey`, `access_key`, `sslpassword`, `auth`; case-insensitive).
  - `redact_text(&str)` scrubs URL-shaped substrings inside arbitrary error text.
  - Apply them to connection error messages surfaced by CLI one-shot JSON errors, `PyO3` exceptions and GUI connect errors.
- **Python:** `PyConnection.connection_url` (`src/lib.rs`) returns the redacted URL. Document it in the Python docs.
- **Tests:** an rstest canary matrix (userinfo, encoded, query-param secrets, malformed URLs, IPv6, mongodb+srv, docker://), and Debug output of each struct never containing a canary.

**W1-T2: secure secret store, migration and `dbcrust secrets`** (large)
- **New module** `src/secrets/` (moves to the `dbcrust-secrets` crate in Wave 2):
  - `SecretBackend` trait: `get/put/delete/list_ids/status`. Typed errors `Unavailable | Locked | Denied | NotFound | Corrupt`.
  - Backends: `KeychainBackend` (keyring: macOS, Windows; service `dbcrust`, account = record id) and `PassphraseFileBackend`.
  - `PassphraseFileBackend` file: `secrets.enc` in the config dir. Format: versioned AEAD envelope; AES-256-GCM (reuse `aes-gcm`); random salt + per-record nonce; AAD = version‖record id; Argon2id KDF, new dep `argon2` 0.5.3, parameters recorded in the header. Unlocked key cached in Linux keyutils with a 30 min timeout; elsewhere process memory only.
  - `MemoryBackend` for tests.
  - `SecretStore` picks the backend from config `secrets.backend = "auto" | "keychain" | "file"`. `auto` = keychain on macOS/Windows, file on Linux. No silent fallback: a locked/unavailable keychain is an error.
  - Values are wrapped in `zeroize::Zeroizing<String>` (zeroize is already in the lockfile). No Serialize/Display on secret types.
- **Record model:**
  - `SecretRecord { v: 1, secret, bound_target: TargetBinding, agent_policy: Option<AgentPolicy>, revision, created_at }`.
  - `TargetBinding { engine, host, port, database, user, tls_mode }`.
  - Record id `conn:<engine>:<user>@<host>:<port>/<db>`. ids are lowercase-normalized; the AI ids are `ai:<provider>` and `oauth:<name>`.
  - Lookup refuses a record whose `bound_target` ≠ the requested target (`TargetMismatch`). It is never silently used.
- **Integration:**
  - `lookup_connection_password` (`src/config.rs:151`) consults the new store first, then legacy `.dbcrust` (read-only), then `.pgpass` / `.my.cnf`.
  - Every save path (`config.rs:4718`, `commands.rs:2520`, `cli_core.rs:1043`) writes to the new store only. `dbcrust_pass::save_password` is no longer called for new saves.
  - `src/ai/key_storage.rs` stores API keys and OAuth tokens in the store. Remove the automatic `ai_keys.enc` fallback; `ai_keys.enc` is read only for migration.
  - Stop creating `enc:` machine-key values anywhere. `password_encryption.rs` keeps decrypt-only for migration (rename the public surface `legacy_decrypt`).
- **CLI:** `dbcrust secrets status|audit|migrate|delete <id>` (usage-rs subcommands in `src/cli.rs`, dispatched like `config`).
  - `status`: backend, availability, lock state, record count. No values.
  - `audit [-o json]`: read-only scan of known locations. Checks plaintext/`enc:` entries in `.dbcrust`, `ai_keys.enc`, secret-ish option values in `sessions.toml`/`recent.toml`, URLs with userinfo in `recent.toml`/history files/named queries, `ssh_password` in config, presence + permissions of `.pgpass`/`.my.cnf`/`.vault-token`. It outputs rule id, path, line, severity and remediation, **never the value**. It never executes password commands and does not follow symlinks.
  - `migrate`: interactive `inquire` flow.
    1. Inventory.
    2. Stage each secret into the store.
    3. Read back and verify.
    4. Only then remove migrated DBCrust-owned legacy entries (`.dbcrust` lines, `ai_keys.enc` entries, `ssh_password`).
    - Third-party files (`.pgpass`, `.my.cnf`) are reported, never edited. Unreadable entries are retained and reported. Idempotent and resumable.
- **Config:** `[secrets] backend` + `file_unlock_timeout_secs`, with `#[serde(default)]`, `FieldSpec` entries and `save_with_documentation`.
- **Docs:** new `docs/src/content/docs/user-guide/secret-storage.md`; update the credential docs that mention `.dbcrust` encryption.
- **Tests:**
  - envelope round-trip, wrong passphrase, corrupt header/record, AAD swap;
  - binding mismatch;
  - migration crash points (simulated by a failing backend at each step) and idempotency;
  - audit corpus never echoing the canary;
  - keyring-backed tests only behind `DBCRUST_TEST_KEYCHAIN=1` (manual, uses a temp macOS keychain).

**W1-T3: agent protocol crate + broker core** (large; root crate `src/broker/` + `crates/agent-protocol`)
- **Protocol (`crates/agent-protocol`, serde/serde_json only):**
  - Frames: u32 big-endian length + JSON, max 16 MiB.
  - Handshake `Hello { protocol: 1, client: String }` → `Welcome { protocol, broker_version }`.
  - Requests:
    - `ListConnections`
    - `ListTables { alias }`
    - `DescribeTable { alias, table }`
    - `Query { alias, sql, max_rows: Option<u32>, timeout_secs: Option<u32> }`
    - `Explain { alias, sql }`
  - Responses:
    - `Connections(Vec<ConnectionSummary { alias, engine, access: ReadOnly|ReadWrite, native_read_only: bool, description }>)`
    - `Result { columns, rows: Vec<Vec<Option<String>>>, row_count, truncated }`
    - `Error { code, message }`
  - Error codes: `unknown_alias`, `denied`, `read_only_violation`, `ddl_refused`, `limit_exceeded`, `timeout`, `query_error`, `connection_error`, `broker_locked`, `protocol_error`.
  - No credential field exists anywhere in the protocol.
- **Broker (`src/broker/`):** `dbcrust broker serve [--mode user|service] [--state-dir] [--socket]`.
  - **State dir:**
    - user mode: `<config dir>/broker/`
    - service mode: `/Library/Application Support/dbcrust-broker` on macOS, `/var/lib/dbcrust-broker` on Linux
    - contents: `aliases.toml` (non-secret descriptors: alias → record id, description), `audit.jsonl`, the store.
  - **Sockets:**
    - query socket: user mode `$XDG_RUNTIME_DIR/dbcrust/broker.sock`, or `<state>/broker.sock` (0600, dir 0700); service mode `/var/run/dbcrust/broker.sock`, mode 0660 group `dbcrust-agents`;
    - admin socket `<state>/admin.sock` (0600, peer uid must be 0 or the service uid).
    - Every accept checks `peer_cred()` against the allowed uids/gids.
  - **Store:** user mode uses `SecretStore` from T2, with records namespaced `broker:<alias>`. Service mode uses `PassphraseFileBackend` / a key file in the state dir (`key_source`).
  - **Enrollment:** `dbcrust broker enroll <alias> <url-without-password> [--allow-writes] [--max-rows N] [--timeout S] [--description]`.
    - The password is read via `inquire::Password`; argv/env passwords are rejected.
    - The broker test-connects, then stores `SecretRecord` with `bound_target` + `AgentPolicy { access, max_rows (default 500, hard cap 10000), timeout_secs (default 30), max_bytes (default 8 MiB) }`.
    - Service mode runs it as `sudo dbcrust broker enroll ...` through the admin socket. User mode writes the store directly.
  - **Other admin commands:**
    - `broker list` (no secrets)
    - `broker revoke <alias>`
    - `broker policy <alias> --max-rows/--timeout/--read-only`: tightening only. Loosening or `--allow-writes` requires re-enroll.
    - `broker status`
    - `broker unlock` (passphrase mode)
  - **Engines accepted for enrollment in v1:** PostgreSQL, MySQL/MariaDB, SQLite, ClickHouse. Others are refused with `unsupported_engine`.
  - **Gateway** (`src/broker/gateway.rs`, the single execution path):
    1. Resolve the alias.
    2. Load the record and verify the binding.
    3. Connect through the existing `Database` with explicit ConnectOptions:
       - read-only connect hardening: PG `default_transaction_read_only`, SQLite `query_only`, MySQL `SET SESSION TRANSACTION READ ONLY`, ClickHouse `readonly=1`;
       - statement timeout;
       - no SSH-tunnel/Vault/docker/env overrides;
       - clear `PG*` / `MYSQL_*` env influence by building options explicitly.
    4. **Classify.** Multi-statement is refused. DDL/DCL/admin (CREATE/ALTER/DROP/TRUNCATE/GRANT/REVOKE/COPY ... PROGRAM, VACUUM, SET ROLE, ATTACH, PRAGMA writes, `SELECT ... INTO`, etc.) is always refused. DML is allowed only if `access = ReadWrite`. Reuse/extend `src/safety.rs`; unknown → refuse.
    5. Execute with auto-LIMIT and **fetch-side** row/byte truncation.
    6. Audit, then respond.
    - Client-requested limits can only tighten the ceiling.
    - Per-alias pool, reused across requests (lazy connect, reconnect on error).
  - **Audit:** `audit.jsonl` lines `{ts, client_label, peer_uid, peer_pid, alias, op, decision, code, sql_sha256, sql_len, rows, bytes, duration_ms}`. It never stores raw SQL text, parameters or results. If audit-start can't be written, the request is refused (fail closed). Size-based rotation (10 MiB × 5).
  - The broker process never logs secrets (relies on T1). It refuses to start in service mode if the state dir/key file permissions are too broad.
- **Tests:**
  - protocol round-trip;
  - frame size limits and malformed frames;
  - gateway classification rstest corpus (CTE writes, `SELECT ... INTO`, multi-statement, comments/quoting tricks, `COPY ... PROGRAM`, PRAGMA, ATTACH);
  - binding mismatch;
  - policy tightening only;
  - audit fail-closed;
  - an end-to-end test over a temp-dir unix socket against an SQLite target with `MemoryBackend`;
  - PG gated on `DATABASE_URL`.

**W1-T4: thin agent client `dbcrust-agent` (CLI + MCP stdio)** (`crates/agent`)
- **Dependencies:** agent-protocol, tokio (rt, net, io-util, macros), serde_json, usage-rs (for CLI consistency). **Must not depend on the root crate**; a test asserts `cargo tree -p dbcrust-agent` has no sqlx/keyring/datafusion.
- **Commands:**
  - `dbcrust-agent list|tables <alias>|describe <alias> <table>|query <alias> -c SQL [-o json|jsonl|csv|table] [--max-rows] [--timeout]|explain <alias> -c SQL|mcp|doctor`.
  - Socket discovery: `--socket`, then `DBCRUST_BROKER_SOCKET` (the path only; no secrets via env), then the service socket, then the user socket.
  - Exit codes mirror the one-shot contract (0/1/2/3/4) + `5 = denied by policy`.
- **MCP server:**
  - Newline-delimited JSON-RPC 2.0, protocol version negotiation (support `2025-06-18` and `2025-03-26`).
  - Tools with JSON Schema inputs. Results as `content: [{type: "text", text: <json envelope>}]` + `structuredContent`; errors as `isError: true` with the code.
  - Nothing other than protocol frames on stdout; logs go to stderr. Bounded line length.
- **Tests:** an MCP conversation fixture (initialize → tools/list → tools/call) against an in-process fake broker; argument validation; oversize rejection.
- **Packaging:**
  - add `--bin dbcrust-agent` to `.github/workflows/build-binaries.yml` archives;
  - add it to the `mise run install` task;
  - add it to `default-members`.

**W1-T5: service-mode install + docs + agent guide** (medium)
- `dbcrust broker install-service [--dry-run]` (needs root):
  - **macOS:** create the hidden `_dbcrust` user/group via `dscl`/`sysadminctl` and the `dbcrust-agents` group, add the invoking user to `dbcrust-agents`, write `/Library/LaunchDaemons/dev.dbcrust.broker.plist`, create the state dir with correct owners/modes, `launchctl bootstrap system`.
  - **Linux:** a systemd unit with `User=dbcrust`, `StateDirectory=dbcrust-broker`, `RuntimeDirectory=dbcrust`, `ProtectHome=yes`, `NoNewPrivileges=yes`, `PrivateTmp=yes`, plus the `dbcrust-agents` group.
  - `--dry-run` prints every command/file without executing.
  - `uninstall-service` mirrors it.
  - Install copies the current binary to a root-owned path (`/usr/local/libexec/dbcrust-broker`), so a user-writable `~/.cargo/bin/dbcrust` can't be swapped under the daemon.
- **Docs:**
  - new `docs/src/content/docs/user-guide/agent-connections.md`: concepts, both modes, the guarantees table from G.3, enrollment, least-privilege DB role recipe for PG/MySQL, MCP setup for Claude Code (`claude mcp add dbcrust -- dbcrust-agent mcp`), Codex (`~/.codex/config.toml`) and Cursor (`mcp.json`), troubleshooting;
  - update `src/agent_guide.md` so agents prefer `dbcrust-agent` for protected connections;
  - update the CLAUDE.md module map;
  - add a README feature bullet.
- **Tests:** generated plist/unit content snapshots; dry-run output; permission checks on fake dirs.

### G.5 Waves 2–4 (roadmap level)

- **Wave 2: build.**
  - **2a quick wins:** cargo features for every existing engine and heavy subsystem (default = all); TLS provider unification; dev debuginfo; measure before and after.
  - **2b crate split** into about 12 crates (G.2/O4), one integrator owning the root manifest, extraction order: core → secrets → drivers (one PR each) → ai/docker → broker. Python/maturin and GUI stay green at every step.
- **Wave 3: connectors**, one crate + feature each, parallel worktrees after 2b:
  1. Redis + SQL Server (`tiberius`)
  2. Redshift (PG-wire adaptation) + libSQL/Turso (HTTP)
  3. shared AWS auth, then S3, DynamoDB and CloudWatch, with InfluxDB alongside
  4. real DuckDB (opt-in, bundled build), then DuckLake

  Each connector declares broker capabilities (native read-only vs. heuristic) before it can be enrolled.
- **Wave 4: hardening.** One-use write approvals with a GUI pending view, Windows broker (named pipes + service), signed broker distribution, systemd encrypted credentials/TPM key source, the empirical macOS keychain ACL test matrix, streaming budgets, and table allowlists for structured operations.

### G.6 Refinements after reviewing W1-T1/T2 (orchestrator)

- **Admin path, v1:**
  - **No admin socket.** Admin commands (`enroll/revoke/policy/list`) write the broker state dir directly. File permissions are the gate: in service mode only root (via `sudo`) or the service user can write `/…/dbcrust-broker`.
  - **Reload:** the broker reloads `aliases.toml` and the store when their mtime changes.
  - **Key source:** service mode supports only `key_source = "file"` in v1. The passphrase-unlocked service store needs an admin channel, so it moves to Wave 4.
- **Connect from the record:** the broker connects **only** from the record's `bound_target`, TLS mode included. `TargetBinding::matches` treats a missing `tls_mode` as a wildcard, which is fine for human lookups. The broker never builds a target from request data.
- **Broker process hardening at start:**
  - `umask 077`; core dumps off (`RLIMIT_CORE = 0`);
  - debugger attach blocked: Linux `prctl(PR_SET_DUMPABLE, 0)`, macOS `ptrace(PT_DENY_ATTACH)`;
  - environment scrubbed: `PG*`, `MYSQL*`, `CLICKHOUSE*`, `SSL_CERT_*`, `*_proxy`/`*_PROXY` and `DBCRUST_*` other than the broker's own;
  - the user's `config.toml` is not loaded in service mode. In user mode, `ssh_tunnel_patterns`, Vault, docker and named-query features are disabled for broker connections.
- **Credential lookups:** in user mode, `broker:` records sit in the same `SecretStore` as human records. The regular CLI only ever looks up `conn:` ids, so broker records are never used outside the broker gateway.

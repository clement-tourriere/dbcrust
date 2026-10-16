# CLAUDE.md

DBCrust — high-performance multi-engine interactive client in Rust with Python bindings. Supports PostgreSQL, MySQL, SQLite, MongoDB, ClickHouse, Elasticsearch, White Dragon, and file formats (Parquet, CSV, JSON) via Apache DataFusion.

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
| `crates/core/` | `dbcrust-core` (lib `dbcrust_core`, layer 0: no driver SDK, HTTP, Docker or TUI deps): `database` (client traits, `ConnectionInfo`, `DatabaseError`), `model`, `options`, `docker_target`, `output_format`, `password_sanitizer`, `safety`, `format`, `complex_display`/`vector_display`/`geojson_display`/`json_display`, `performance_analyzer`, `regex_operators`, `sql_parser`, `sql_buffer` (lexer/splitter; the reedline `SqlValidator` stays in root), `explain_plan`, `ai_config`, `tls` (feature `tls`). Cargo features `usage` (derive `ValueEnum` on `OutputFormat`) and `tls`; root enables both and re-exports every module under its old path (`crate::database::…`, `dbcrust::format::…`); `database::create_database_client` is added by root's `database` facade because the factory lives in `src/drivers.rs`. `regex`/`serde_json` feature sets are declared explicitly so core works standalone (`cargo test -p dbcrust-core`) |
| `src/main.rs` | Entry point, Tokio runtime, CLI orchestration |
| `src/lib.rs` | PyO3 bindings (`PyDatabase`, `PyConfig`, `run_cli_loop`); `ai_config_status`/`run_ai_investigation` stay registered as stubs without the `ai` feature |
| `src/features.rs` | `FeatureNotCompiled` (`Docker`/`Ai`/`ExplainTui`/`Clipboard`): the typed "not compiled into this build" error of the add-on features |
| `src/cli_core/ai.rs` | `??`, `???` and the `\ai` wizards (`impl CliCore`, compiled only with feature `ai`) |
| `src/commands.rs` | **Enum-based command system** — all `\cmd` logic lives here |
| `src/cli.rs` | Arg parsing via `usage-rs` derive (jdx/usage) — private `Root` wraps public `Args`; one-shot flags (`-c`/`-f`, `-o/--format`, `--read-only`, `--no-input`); `OutputFormat`/`OneShotSource`/`ParseExit` live here; hidden `__complete_word__`/`__usage_spec__` endpoints answered in `parse_from_argv` |
| `src/shell_completion.rs` | Rust-side URL completer (`complete_connection_url`: schemes, sessions, docker, files) + completion-script generation (usage scripts call the binary back at Tab time) |
| `crates/core/src/safety.rs` | Read-only statement classification — `--read-only` guard + AI agent guard share it; `classify_for_broker` is the broker's lexer-based classifier (default-deny) |
| `src/agent_guide.md` | Embedded `dbcrust agents` guide (the one-shot contract for AI coding agents) |
| `crates/ai/` | `dbcrust-ai` (lib `dbcrust_ai`, layer 2: core [`tls`] + `dbcrust-secrets` + genai/reqwest; no driver SDK, Docker, TUI or root): the AI assistant (`??` text-to-SQL, `\ai`) — multi-provider via `genai` (`reqwest_genai` = its reqwest 0.13 with `rustls-no-provider`; the `genai =0.7.0-beta.2` pin lives in this manifest), ChatGPT sign-in, key storage via `dbcrust-secrets`. Talks to the database only through the `AiSession` trait (`session.rs`); root implements it for `Database` in `src/ai_session_impl.rs` and re-exports the crate as `dbcrust::ai` (`ai::AdapterKind` re-exports genai's provider enum so root has no genai dependency). Config types live in `crates/core/src/ai_config.rs` (`ai::config`). Has its own `test_isolation` canaries (dev-dependency `dbcrust-secrets/test-mode`) and the genai TLS regression tests (`tls_tests.rs`) |
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
| `crates/driver-white-dragon/` | `dbcrust-driver-white-dragon` (`WhiteDragonClient`): SQL/qualifier HTTP adapter with generic WDSP schema introspection; may depend on `dbcrust-core` and its SDK (reqwest) only |
| `crates/driver-postgres/` | `dbcrust-driver-postgres` (`PostgreSQLClient`): sqlx `postgres` + pgvector; `format_postgresql_value` lives here; the global complex/vector display configs stay core statics |
| `crates/driver-mysql/` | `dbcrust-driver-mysql` (`MySqlClient`): sqlx `mysql`; the `.cargo/audit.toml` RSA exception covers this crate's sqlx-mysql |
| `crates/driver-clickhouse/` | `dbcrust-driver-clickhouse` (`ClickHouseClient`): `clickhouse` crate exactly pinned to `=0.15.1` (in `[workspace.dependencies]`; see Rust dependency policy) + reqwest for the HTTP calls |
| `crates/driver-elasticsearch/` | `dbcrust-driver-elasticsearch` (`ElasticsearchClient`): `elasticsearch` SDK (`elasticsearch::http` is an SDK module) + regex (default features: `(?i)` needs `unicode-case`) |
| `crates/driver-mongodb/` | `dbcrust-driver-mongodb` (`MongoDBClient`): `mongodb` + `bson` SDKs |
| `crates/driver-files/` | `dbcrust-driver-files` (`DataFusionClient`, serves Parquet/CSV/JSON/DuckDB): Apache DataFusion (`datafusion::arrow`, `datafusion::sql::sqlparser` re-exports; `sqlparser` stays a single 0.61.0 shared with core) |
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

Root `default = ["bundled"]` ships every engine and add-on (owner decision D3); `standard` = `postgres` + `mysql` + `sqlite`. Engine features (`postgres`, `mysql`, `sqlite`, `mongodb`, `clickhouse`, `elasticsearch`, `white-dragon`, `files`) forward to `dbcrust-drivers/<x>`; add-ons are `docker`, `ai`, `explain-tui`, `clipboard`. `DatabaseType` variants and config parsing never depend on features.

- Gate call sites with `#[cfg(feature = "…")]`; a disabled add-on answers `crate::features::FeatureNotCompiled`, a disabled engine `DatabaseError::ConnectorNotBuilt`. Commands keep their `Command` variants (so `\ai`/`\docker` explain themselves instead of "unknown command").
- Hide disabled engines from completion/help with `dbcrust_drivers::is_compiled` / `UrlScheme::is_available`; never hardcode lists.
- **New connector** = driver crate + optional dep and feature in `dbcrust-drivers` + one `connectors!` row + root feature forwarding to it + a decision on `bundled`: light connectors are added to `bundled` explicitly (changelog note), heavy ones (DuckDB, AWS) stay opt-in. Add it to the CI matrix (`rust-features`) and `mise run check:features`.
- Verify with `mise run check:features` (zero-engine, `standard`, each feature alone) and `cargo test -p dbcrust --no-default-features --features standard`; tests that need a disabled engine/add-on are `#[cfg]`-gated, not deleted. Release binaries, wheels and gui-core use `bundled` explicitly.

## Rust dependency policy

- Do not run blanket dependency upgrades. Review release notes, provenance/maintainer changes, RustSec advisories, and newly introduced transitive crates; keep breaking migrations separate.
- New crate releases must be at least **24 hours old**, including security updates. Use `cargo update -p CRATE --precise VERSION`, then `python3 .github/scripts/check_cargo_cooldown.py --base HEAD` before compiling (Python 3.11+). CI checks lockfile additions against the PR base, including transitive and target-only entries, with no security-PR exemption.
- Use `--locked` for validation/builds. Run `cargo audit`, `cargo machete --with-metadata`, and `cargo +nightly udeps --workspace --all-targets --all-features --locked` when reviewing removals. `prettytable-rs` is imported as `prettytable`; never remove it based on a name-only false positive.
- `.cargo/audit.toml` documents the narrowly justified SQLx/MySQL RSA exception; do not suppress additional advisories to make CI green. See [dependency review notes](docs/src/content/docs/user-guide/development.md#rust-dependency-updates) for residual upstream warnings.
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

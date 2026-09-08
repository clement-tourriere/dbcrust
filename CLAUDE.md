# CLAUDE.md

DBCrust — high-performance multi-engine interactive client in Rust with Python bindings. Supports PostgreSQL, MySQL, SQLite, MongoDB, ClickHouse, Elasticsearch, White Dragon, and file formats (Parquet, CSV, JSON) via Apache DataFusion.

## Build Commands

`mise.toml` requires mise 2026.8.16+ and routes Cargo through the pinned Mr Boxington build cache. Run `mise install` once; use `mise run` tasks (or an activated mise shell) so Cargo is wrapped. Diagnose cache setup with `mbx doctor`.

```bash
mise install
mise run build:dev        # dev build
mise run build            # release build
mise run install          # install dbcrust + dbc into ~/.cargo/bin
mise run test             # run tests
cargo test -- --nocapture # tests with output
cargo run -- postgres://localhost/test  # run CLI directly

# Python bindings
mise run py:dev           # maturin dev build
mise run py:build         # wheel

# GUI (Tauri)
mise run gui:install && mise run gui:build
```

## Module Map

| File | Purpose |
|------|---------|
| `src/main.rs` | Entry point, Tokio runtime, CLI orchestration |
| `src/lib.rs` | PyO3 bindings (`PyDatabase`, `PyConfig`, `run_cli_loop`) |
| `src/commands.rs` | **Enum-based command system** — all `\cmd` logic lives here |
| `src/cli.rs` | Arg parsing via `usage-rs` derive (jdx/usage) — private `Root` wraps public `Args`; one-shot flags (`-c`/`-f`, `-o/--format`, `--read-only`, `--no-input`); `OutputFormat`/`OneShotSource`/`ParseExit` live here; hidden `__complete_word__`/`__usage_spec__` endpoints answered in `parse_from_argv` |
| `src/shell_completion.rs` | Rust-side URL completer (`complete_connection_url`: schemes, sessions, docker, files) + completion-script generation (usage scripts call the binary back at Tab time) |
| `src/safety.rs` | Read-only statement classification — `--read-only` guard + AI agent guard share it |
| `src/agent_guide.md` | Embedded `dbcrust agents` guide (the one-shot contract for AI coding agents) |
| `src/ai/` | AI assistant (`??` text-to-SQL, `\ai`) — multi-provider via `genai` |
| `src/update.rs` | `--update`: install-channel detection + GitHub release check |
| `src/config.rs` | TOML config, `save_with_documentation` must be updated for new fields |
| `src/config_editor.rs` | Schema-driven `\config` menu/get/set + SSH tunnel manager (`dbcrust config` CLI) |
| `src/prompt.rs` | Reedline interactive REPL |
| `src/completion.rs` | SQL autocomplete with metadata caching |
| `src/format.rs` | Output formatting (table, expanded, CSV, JSON, JSONL) — `render_query_results` is the dispatch |
| `src/database.rs` | `DatabaseClient` + `MetadataProvider` traits |
| `src/database_postgresql.rs` | PG implementation (`format_postgresql_value` ~line 1390) |
| `src/database_datafusion.rs` | File format queries (Parquet, CSV, JSON) |
| `src/database_white_dragon.rs` | White Dragon SQL/qualifier HTTP adapter with generic WDSP schema introspection |
| `src/ssh_tunnel.rs` | SSH tunnel management |
| `src/vault_client.rs` | HashiCorp Vault credentials |
| `src/named_queries.rs` | Parameterized queries (`$1`, `$*`, `$@`) |

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
| Universal passwords | `~/.dbcrust` (format: `db_type:host:port:db:user:pass`) |

### 5. Testing

- Use `rstest` for parameterized tests.
- All Rust tests are inline `#[cfg(test)]` modules — there is no `tests/` directory.
- Tests auto-isolate config to `/tmp/dbcrust_test_{pid}/` (heuristic: thread name contains "test" or `RUST_TEST_MODE` is set — `cargo test -- --test-threads=1` runs on the main thread and escapes it; export `RUST_TEST_MODE=1` in that case).
- PostgreSQL integration tests gate on `DATABASE_URL` env var and skip gracefully; other backends have no DB-gated tests yet.
- Python: `mise run py:test` runs `python/dbcrust/django/tests` (pure Python — the repo-root `conftest.py` stubs the native module when it isn't built).

### 6. Error handling

- `thiserror` for custom error types.
- PostgreSQL type decode: use `.or_else(|_| handle_custom_postgresql_type(...))` fallback, not `.map_err(...)`.

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

# CLAUDE.md

DBCrust — high-performance multi-database interactive client in Rust with Python bindings. Supports PostgreSQL, MySQL, SQLite, MongoDB, ClickHouse, Elasticsearch, and file formats (Parquet, CSV, JSON) via Apache DataFusion.

## Build Commands

```bash
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
| `src/cli.rs` | Clap arg parsing (no args → prints help; `--update` self-updates) |
| `src/ai/` | AI assistant (`??` text-to-SQL, `\ai`) — multi-provider via `genai` |
| `src/update.rs` | `--update`: install-channel detection + GitHub release check |
| `src/config.rs` | TOML config, `save_with_documentation` must be updated for new fields |
| `src/prompt.rs` | Reedline interactive REPL |
| `src/completion.rs` | SQL autocomplete with metadata caching |
| `src/format.rs` | Output formatting (table, expanded, JSON, CSV) |
| `src/database.rs` | `DatabaseClient` + `MetadataProvider` traits |
| `src/database_postgresql.rs` | PG implementation (`format_postgresql_value` ~line 1390) |
| `src/database_datafusion.rs` | File format queries (Parquet, CSV, JSON) |
| `src/ssh_tunnel.rs` | SSH tunnel management |
| `src/vault_client.rs` | HashiCorp Vault credentials |
| `src/named_queries.rs` | Parameterized queries (`$1`, `$*`, `$@`) |

## Connection URL Types

```
postgres://user:pass@host:5432/db?sslmode=require
mysql://  sqlite:///path  mongodb://  clickhouse://
session://name          # saved session
recent://               # interactive picker
docker://container/db
vault://role@mount/database
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

### 3. Config — `#[serde(default)]` + update `save_with_documentation`

- New config fields require `#[serde(default)]` for backward compatibility.
- Always update `save_with_documentation()` in `src/config.rs` so the field appears with comments in generated configs.

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

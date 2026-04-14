---
title: "Development"
---

# Development

This page covers how to build, test, and contribute to DBCrust.

## Prerequisites

- **[Rust](https://rustup.rs/)** — stable toolchain
- **[mise](https://mise.jdx.dev/)** — manages Bun, commitizen, and other tools automatically
- **Python 3.10+** — only needed for the Python bindings

```bash
# Install mise (if you don't have it)
curl https://mise.run | sh

# From the project root — install all managed tools
mise install
```

`mise install` reads `mise.toml` and sets up:

| Tool | Purpose |
|------|---------|
| **Bun** | JavaScript runtime for the GUI frontend |
| **commitizen** | Conventional commit helper |
| **pkl** | Configuration language |
| **hk** | Git hooks |

## Building

### CLI

```bash
mise run build:dev      # debug build (fast compile)
mise run build          # release build (optimized)
cargo run -- <url>      # run directly without installing
cargo install --path .  # install to ~/.cargo/bin
```

The binary is named `dbcrust`. A short alias `dbc` is also built (see `src/bin/dbc.rs`).

### GUI

```bash
mise run gui:install        # install frontend deps via Bun
mise run gui:dev            # dev mode (Vite hot-reload + Tauri)
mise run gui:build          # production build with installers
mise run gui:frontend       # frontend dev server only (no Tauri)
mise run gui:build-frontend # build frontend only
mise run gui:build-rust     # build Tauri Rust backend only
```

See [Desktop GUI](gui.md) for full details.

### Python bindings

```bash
mise run py:dev       # maturin develop (editable install)
mise run py:build     # build wheel
mise run py:test      # run pytest
pip install -e ./python  # alternative: pip editable install
```

## Testing

```bash
mise run test                    # all tests
cargo test -- --nocapture        # with stdout
cargo test test_name             # specific test
cargo test --lib module_name     # specific module
cargo test --test "*"            # integration tests only
mise run py:test                 # Python tests
```

## Linting and formatting

```bash
mise run fmt     # cargo fmt
mise run lint    # clippy (correctness, suspicious, perf as errors; style, complexity as warnings)
mise run check   # fmt + lint + test in sequence
```

## Task reference

All tasks are defined in `mise.toml`. Run `mise tasks` to list them.

| Task | Description |
|------|-------------|
| `build` | `cargo build --release` |
| `build:dev` | `cargo build` |
| `test` | `cargo test` |
| `lint` | Clippy with strict settings |
| `fmt` | `cargo fmt` |
| `check` | fmt → lint → test |
| `gui:install` | Bun install for frontend |
| `gui:dev` | Dev mode (frontend + Tauri) |
| `gui:build` | Production GUI build |
| `gui:frontend` | Vite dev server only |
| `gui:build-frontend` | Build frontend only |
| `gui:build-rust` | Build Tauri backend only |
| `gui:build-rust-release` | Release build of Tauri backend |
| `gui:lint` | Clippy on GUI crate |
| `gui:clean` | Remove GUI build artifacts |
| `py:dev` | `maturin develop` |
| `py:build` | `maturin build --release` |
| `py:test` | `python -m pytest` |
| `all:build` | Build CLI + GUI + Python |
| `all:clean` | Clean everything |

## Project layout

```
├── src/                       # Rust CLI + library
│   ├── main.rs                # tokio entry point
│   ├── lib.rs                 # public API + PyO3 bindings
│   ├── commands.rs            # backslash command enum (strum-driven)
│   ├── cli.rs                 # clap argument parsing
│   ├── cli_core.rs            # REPL loop and command dispatch
│   ├── config.rs              # TOML config + session + named query storage
│   ├── database.rs            # DatabaseClient / MetadataProvider traits
│   ├── database_postgresql.rs # PostgreSQL implementation
│   ├── database_mysql.rs      # MySQL implementation
│   ├── database_sqlite.rs     # SQLite implementation
│   ├── database_clickhouse.rs # ClickHouse implementation
│   ├── database_mongodb.rs    # MongoDB implementation
│   ├── database_elasticsearch.rs # Elasticsearch implementation
│   ├── database_datafusion.rs # Parquet/CSV/JSON via DataFusion
│   ├── completion.rs          # SQL autocompletion engine
│   ├── command_completion.rs  # backslash command completion
│   ├── format.rs              # output formatting (table, expanded, psql)
│   ├── prompt.rs              # reedline custom prompt
│   ├── highlighter.rs         # SQL syntax highlighting
│   ├── ssh_tunnel.rs          # SSH tunnel management
│   ├── vault_client.rs        # Vault HTTP client
│   ├── vault_encryption.rs    # Vault credential cache encryption
│   ├── docker.rs              # Docker container discovery (bollard)
│   ├── named_queries.rs       # named query parameter substitution
│   ├── pgpass.rs              # .pgpass file support
│   ├── dbcrust_pass.rs        # .dbcrust password file (all databases)
│   ├── password_encryption.rs # AES-256-GCM password encryption
│   ├── password_sanitizer.rs  # URL password redaction
│   ├── history_manager.rs     # per-session history
│   ├── performance_analyzer.rs # query performance analysis
│   ├── script.rs              # external editor integration
│   ├── pager.rs               # output paging
│   ├── logging.rs             # tracing setup
│   ├── explain_tui/           # interactive EXPLAIN TUI (ratatui + crossterm)
│   │   ├── mod.rs
│   │   ├── app.rs
│   │   ├── plan_tree.rs
│   │   └── ui.rs
│   ├── sql_parser.rs          # SQL parsing
│   ├── sql_parser_postgresql.rs
│   ├── sql_parser_mysql.rs
│   ├── sql_parser_sqlite.rs
│   ├── sql_parser_trait.rs
│   ├── sql_context.rs         # SQL context for autocompletion
│   ├── url_scheme.rs          # URL scheme parsing and normalization
│   ├── shell_completion.rs    # shell completion generation (bash/zsh/fish/powershell)
│   ├── complex_display.rs     # complex type rendering
│   ├── json_display.rs        # JSON pretty display
│   ├── geojson_display.rs     # GeoJSON rendering
│   ├── vector_display.rs      # pgvector display
│   └── myconf.rs              # MySQL .my.cnf support
├── gui/                       # Tauri desktop app
│   ├── src/                   # React + TS frontend
│   ├── src-tauri/             # Tauri Rust backend
│   └── package.json           # Bun-managed deps
├── python/                    # Python package (PyO3 + maturin)
├── docs/                      # MkDocs Material documentation
├── mise.toml                  # tool + task definitions
├── Cargo.toml                 # workspace root
├── pyproject.toml             # Python package metadata
└── mkdocs.yml                 # docs site config
```

## Architecture notes

- **Command system**: All backslash commands are variants of a `Command` enum. The `CommandShortcut` enum (with `strum::EnumIter`) auto-generates help text, completion, and dispatch. Never use hardcoded arrays for command lists.
- **Database abstraction**: The `DatabaseClient` and `MetadataProvider` traits in `database.rs` define the interface. Each database has its own implementation file.
- **Async**: Database operations use `tokio` + `async-trait`. Shared state uses `Arc<Mutex<T>>`.
- **GUI bridge**: The Tauri backend in `gui/src-tauri/src/lib.rs` wraps `dbcrust` core functions as `#[tauri::command]` handlers. Database operations run on dedicated threads with `LocalSet` to handle `!Send` futures.
- **Error handling**: `thiserror` for custom error types. User-facing prompts use `inquire`.
- **Config**: `serde` with `#[serde(default)]` for backward compatibility. Separate TOML files for settings, sessions, named queries, and recent connections.

## Conventional commits

The project uses [Commitizen](https://commitizen-tools.github.io/commitizen/) for conventional commits:

```bash
cz commit    # interactive commit
cz bump      # bump version based on commit history
```

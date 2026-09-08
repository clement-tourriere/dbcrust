---
title: "Development"
---

# Development

This page covers how to build, test, and contribute to DBCrust.

## Prerequisites

- **[Rust](https://rustup.rs/)** — stable toolchain
- **[mise](https://mise.jdx.dev/) 2026.8.16+** — manages Bun, Commitizen, and other tools automatically
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
| **Commitizen** | Conventional commit helper |
| **Mr Boxington** | Shared, self-pruning Rust build cache |
| **pkl** | Configuration language |
| **hk** | Git hooks |

## Rust build cache

The project-scoped mise wrapper routes Cargo through [Mr Boxington](https://mr-boxington.jdx.dev/). `mise run` tasks use it automatically; activate mise in your shell if you also want plain `cargo` commands to use the cache.

```bash
mbx doctor          # verify Cargo and cache integration
mbx cache stats     # inspect cache usage
mbx gc --dry-run    # preview automatic cleanup
```

Set `MBX_DISABLE=1` for a one-off uncached Cargo command.

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

## Rust dependency updates

Treat updates as code changes, not automatic trust decisions. Review upstream
release notes, repository/maintainer changes, advisories, and added transitive
crates before building. Avoid blanket `cargo update`; review breaking upgrades
separately. A cooldown reduces exposure to freshly published compromises but
cannot prove a package is safe.

Every new release must be **at least 24 hours old**, including security fixes.
Weekly Dependabot version PRs have a one-day cooldown and are not auto-merged.
Because Dependabot exempts security PRs, CI also checks every new external
`Cargo.lock` entry against the PR base (or the previous push), including
transitive, optional, and platform-specific dependencies. The guard verifies
crates.io publication timestamps, checksums, and yank status; missing metadata
or new non-crates.io sources fail closed. Retry a failed check after the cooldown
expires or the registry recovers. Make **Rust · dependency audit** a required
check in branch protection to prevent merging around it.

From the project root, using Python 3.11+ for the stdlib-only guard:

```bash
# After reviewing a specific upstream release and its publication date:
cargo update -p CRATE --precise VERSION
python3 .github/scripts/check_cargo_cooldown.py --base HEAD
cargo audit
cargo machete --with-metadata
cargo +nightly udeps --workspace --all-targets --all-features --locked
cargo test --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
```

For committed changes use `--base origin/main` (fetch that ref first). Use
`--locked` when validating/building so Cargo cannot silently resolve newer,
unreviewed versions. Both workspace members share the root lockfile. The guard
does not replace review or constrain arbitrary local `cargo update` commands.

### Review snapshot — 2026-09-08

- Selected updates include PyO3 **0.29.2** (with binding migration), Ratatui
  **0.30.2**, usage-rs **6.8.0**, MongoDB **3.9.0**, DataFusion **53.1.0**, and
  Tauri **2.11.5**, plus compatible security fixes. All **160 new registry
  versions** passed the one-day age check; none were yanked.
- Fixed **14 vulnerability findings** affecting HTTP/2, Python bindings, XML,
  QUIC, archive validation, TLS certificate validation, and time parsing.
  Removed four unsoundness warnings and the yanked `spin` version as well.
- `cargo audit` passes with one documented exception: **RUSTSEC-2023-0071**
  (`rsa` via SQLx/MySQL). There is no patched release; this code path only
  encrypts credentials with a public key, not the vulnerable private-key
  operations. Reassess the exception when that dependency tree changes.
- **20 informational warnings remain**, predominantly inherited from Tauri's
  GTK3 dependencies, plus unmaintained `paste`, `fxhash`, and Unicode helpers.
  These include unsoundness advisories for `glib` and build-time `rand 0.7.3`.
  They are not silently suppressed or claimed fixed. `cargo deny check` is not
  a passing gate: the project has no license allowlist/deny configuration, and
  deny also reports the remaining advisories.
- No unused direct dependencies were found by `cargo machete --with-metadata`
  or workspace/all-feature `cargo udeps` on macOS. `prettytable-rs` is used under
  the library name `prettytable`. Platform-specific usage still needs review
  before any removal.
- Deferred `genai 0.7.0-beta.23` (published that day) and breaking updates such
  as SQLx 0.9, DataFusion 55, BSON 3, Reqwest 0.13, and keyring 4.

### 32-bit release compatibility

The `v0.37.1` release attempt exposed a ClickHouse 0.15.2 regression: its new
Native reader unconditionally compiles `const SAFE_ALLOCATION_LIMIT: usize =
1 << 32`, which overflows on i686 even though DBCrust does not use the Native
API. ClickHouse is therefore exactly pinned to **0.15.1** (published 2026-06-04,
not yanked). The remaining security updates are retained; the restored
transitive dependencies are checked by the same cooldown/audit policy.

CI now checks the actual Python extension library for
`i686-unknown-linux-gnu`, not just 64-bit hosts. Confirm **Rust · 32-bit Python
compatibility** passes before tagging a dependency release. Keep the pin until
an upstream fix passes that check; do not drop the i686 wheel to hide a failure.

To reproduce the check on an x86_64 Debian/Ubuntu development host:

```bash
sudo apt-get install gcc-multilib g++-multilib
rustup target add i686-unknown-linux-gnu
PYO3_NO_PYTHON=1 cargo check --locked -p dbcrust --lib --features python \
  --target i686-unknown-linux-gnu
```

`PYO3_NO_PYTHON=1` uses the configured `abi3-py310` ABI instead of trying to
use a 64-bit Python interpreter for the 32-bit target. This is a compile check;
the release workflow still builds and packages the real i686 wheel.

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
│   ├── cli.rs                 # argument parsing (usage-rs derive)
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

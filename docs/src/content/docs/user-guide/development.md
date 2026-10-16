---
title: "Development"
---

# Development

This page covers how to build, test, and contribute to DBCrust.

## Prerequisites

- **[Rust](https://rustup.rs/)** — stable toolchain
- **[mise](https://mise.jdx.dev/) 2026.8.16+** — manages Commitizen, the Mr Boxington build cache and the other tools, and runs every task. You don't have to install it: the repository's `./bin/mise` wrapper downloads a pinned mise on first use.
- **Python 3.10+** — only needed for the Python bindings

```bash
# From the project root — install all managed tools
mise install

# No mise? Use the committed wrapper the same way, for every command on this page
./bin/mise install
./bin/mise run build:dev
```

On first use in a clone, mise asks you to trust `mise.toml` (or run `mise trust`). `./bin/mise` keeps its mise binary and tools in mise's usual directories, so they are shared with a mise you install later. On Windows, run `bin\mise.cmd`.

`mise install` reads `mise.toml` and sets up:

| Tool | Purpose |
|------|---------|
| **Mr Boxington** (`mbx`) | Shared Cargo build cache — see [Build cache](#build-cache-mr-boxington) |
| **Bun** | JavaScript runtime for the documentation site |
| **Commitizen** | Conventional commit helper |
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

### Desktop app

```bash
mise run gui:dev            # run the app (dev build, optimised dependencies)
mise run gui:build          # release build
mise run gui:test           # backend tests (dbcrust-gui-core, no GPUI needed)
mise run gui:lint           # clippy on both GUI crates
mise run gui:screenshots    # regenerate the docs/README screenshots headlessly
mise run gui:bundle         # macOS DBCrust.app
```

The app is two crates: `gui/core` (`dbcrust-gui-core`, plain Rust — workers, storage, models) and `gui` (`dbcrust-gui`, the GPUI Kit views). See `gui/ARCHITECTURE.md`.

See [Desktop GUI](gui.md) for full details.

### Python bindings

```bash
mise run py:dev       # maturin develop (editable install)
mise run py:build     # build wheel
mise run py:test      # run pytest
pip install -e ./python  # alternative: pip editable install
```

### Build features

The root crate (`dbcrust`) is split into Cargo features so a build only contains the engines and add-ons it needs. **The default build is `bundled`**: every engine and add-on DBCrust has shipped so far. Release binaries, Python wheels and the desktop app all use `bundled`.

| Feature | What it enables | Pulls in |
|---------|-----------------|----------|
| `bundled` (default) | Everything below | — |
| `standard` | `postgres` + `mysql` + `sqlite` (the slim preset) | sqlx |
| `postgres` | PostgreSQL | sqlx, pgvector |
| `mysql` | MySQL / MariaDB | sqlx |
| `sqlite` | SQLite | sqlx (bundled libsqlite3) |
| `mongodb` | MongoDB | `mongodb`, `bson` |
| `clickhouse` | ClickHouse | `clickhouse` (pinned `=0.15.1`) |
| `elasticsearch` | Elasticsearch | `elasticsearch` SDK |
| `white-dragon` | White Dragon search SQL over HTTP | reqwest only |
| `redis` | Redis / Valkey (`redis://`, `rediss://`) — **in `bundled`, not in `standard`** | `redis` (tokio + rustls/ring) |
| `mssql` | SQL Server / Azure SQL (`mssql://`, `sqlserver://`) — **in `bundled`, not in `standard`** | vendored `tiberius` 0.13.0 (`third_party/tiberius`: rustls on ring only, see below) |
| `redshift` | Amazon Redshift (`redshift://`) — **in `bundled`, not in `standard`** | `sqlx` PostgreSQL protocol (no extra crate) |
| `libsql` | libSQL / Turso over HTTP (`libsql://`, `turso://`, `libsql+http://`) — **in `bundled`, not in `standard`** | `reqwest` only (hand-written Hrana client) |
| `influxdb` | InfluxDB v1/v2/v3 (`influxdb://`, `influxdb2://`, `influxdb3://`) — **in `bundled`, not in `standard`** | reqwest (rustls/ring) and `csv-core`, both already in the tree |
| `files` | Parquet, CSV, JSON and DuckDB files | Apache DataFusion / Arrow / Parquet |
| `docker` | `docker://` URLs, `\docker`, container completion | bollard |
| `ai` | `??`, `???`, `\ai`, the Python AI functions | genai, reqwest 0.13 |
| `explain-tui` | `\x` plan TUI and the `\sv` schema browser | ratatui, textwrap |
| `clipboard` | `\ecopy` | arboard |
| `python` | PyO3 bindings (maturin passes it) | pyo3 |

```bash
# the default, everything
cargo install --path . --locked

# the slim preset: PostgreSQL, MySQL, SQLite
cargo install --path . --locked --no-default-features --features standard
mise run build:slim      # release build of the slim preset
mise run install:slim    # install it

# a custom build: SQLite and ClickHouse with Docker discovery
cargo build --no-default-features --features sqlite,clickhouse,docker

# type-check the whole matrix (what CI does)
mise run check:features
```

Cost of each feature, as unique crates added to the normal dependency tree (`cargo tree -p dbcrust --no-default-features --features <x> -e normal`), against 216 crates for the zero-engine build:

| Feature | Added crates | | Feature | Added crates |
|---------|-------------:|-|---------|-------------:|
| `files` | +107 | | `ai` | +34 |
| `mongodb` | +53 | | `explain-tui` | +22 |
| `mysql` | +46 | | `clickhouse` | +16 |
| `postgres` | +35 | | `elasticsearch` | +15 |
| `sqlite` | +21 | | `redis` | +12 |
| `docker`, `clipboard` | +7 each | | `mssql` | +17 |

`standard` is 272 crates against 487 for `bundled` (`redshift` and `libsql` add no crate: they share `sqlx` with `postgres` and `reqwest` with the HTTP engines, see below); the dev-profile binary drops from about 327 MB (201 MB stripped) to 77 MB (49 MB stripped). DataFusion is by far the heaviest piece, which is why `files` is the first thing to drop.

`mssql` adds 7 crates to the lockfile (`tiberius`, `tiberius-macros`, `asynchronous-codec`, `connection-string`, `futures-timer`, `pretty-hex`, `secrecy`), roughly +1.2 MB of stripped release binary, and no C build: upstream `tiberius` 0.13.0 hard-codes the `aws-lc-rs` TLS provider (a cmake/C compile) and 0.12.3 sits on a rustls with three open RustSec advisories, so DBCrust vendors 0.13.0 with a four-line patch that selects `ring` like the rest of the tree. `third_party/tiberius/DBCRUST_PATCH.md` records the original archive checksum, the exact diff, how to re-vendor on a release and the text of the upstream proposal; `cargo tree --locked -i aws-lc-rs` must keep reporting nothing.

`influxdb` adds no registry crate to the lockfile: the HTTP client is hand-written on the `reqwest` 0.12 the other HTTP drivers use, and the Flux CSV is read with `csv-core`, already locked through DataFusion's `csv`. The `influxdb` crate was rejected because it wraps reqwest 0.13 with the aws-lc provider and speaks neither Flux nor v3. Its cost in the normal dependency tree is 2 crates (the driver and `csv-core`).

A disabled engine is still a `DatabaseType`: config files, sessions and `recent.toml` written by a full build keep parsing. Connecting to it fails with a message that names the feature, e.g. *"MongoDB support is not compiled into this build (enable the `mongodb` cargo feature, or install a full build)"*; `dbcrust-drivers` exposes `is_compiled`, `compiled_engines` and `required_feature` for code that has to adapt. URL-scheme completion only offers the schemes the build can serve, and the credential broker refuses to enrol an engine the build does not contain (`unsupported_engine`, same hint).

#### Adding a connector

1. A `dbcrust-driver-<name>` crate (depends on `dbcrust-core` and its SDK only).
2. In `dbcrust-drivers`: an optional dependency, a same-named feature, and **one row** in the `connectors!` table (`"<feature>" => Engine => path::Client;`). The `match`es have no wildcard, so a missing row is a compile error.
3. In the root `Cargo.toml`: `<name> = ["dbcrust-drivers/<name>"]`.
4. Decide where it lives:
   * **Light** (small SDK, no heavy transitive tree): add it to `bundled` explicitly, with a changelog note, so the default build gains it.
   * **Heavy** (DuckDB, AWS SDKs …): leave it out of `bundled`; users opt in with `--features <name>`. Never add a connector to `bundled` silently.
5. Add it to the matrix in `.github/workflows/ci.yml` (`rust-features`) and to `mise run check:features`.

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

## Build cache (Mr Boxington)

Cargo commands run through mise — for the CLI, the desktop app and the Python bindings alike — go through [Mr Boxington](https://mr-boxington.jdx.dev/) (`mbx`), the Cargo build cache by mise's author. `mise.toml` installs a pinned `mbx` and declares a project-scoped `cargo` wrapper (`[wrappers.cargo]`, mise 2026.8.16 or newer — `./bin/mise` already is), so you keep typing plain `cargo` commands:

- Compilations are cached in one store and reused across worktrees and branches: building one checkout warms the next.
- `target/` becomes a link to a managed directory with a disk budget; old targets and cache entries are collected automatically, and more aggressively when the disk runs low.
- Builds running at the same time share one CPU and memory budget.

Cargo uses mbx when it runs through mise: `mise run` tasks, `mise exec -- cargo …`, or a shell with mise activation or shims on `PATH`. Check with `mise exec -- mbx doctor`. Editors and coding agents that call `~/.cargo/bin/cargo` directly bypass the cache: point them at `mise exec`, and see [rust-analyzer setup](https://mr-boxington.jdx.dev/setup#rust-analyzer) to route editor checks through mbx.

```bash
mise run cache:stats            # savings and disk use
mise run cache:gc -- --dry-run  # preview what collection would reclaim
mise run cache:gc               # reclaim space now
```

CI uses the same cache through [`jdx/mr-boxington-action`](https://github.com/jdx/mr-boxington-action), pinned to a commit and an `mbx` version. Release builds don't use it.

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
| `build:slim` | Release build of the slim preset (`--no-default-features --features standard`) |
| `install:slim` | Install the slim preset into `~/.cargo/bin` |
| `test` | `cargo test` |
| `lint` | Clippy with strict settings |
| `fmt` | `cargo fmt` |
| `check` | fmt → lint → test |
| `check:features` | `cargo check` of the zero-engine build, `standard` and each feature alone |
| `gui:dev` | Run the desktop app (dev build, optimised dependencies) |
| `gui:build` | Optimised build of the desktop app (`target/gui-release`) |
| `gui:install` | Install `dbcrust-gui` into `~/.cargo/bin` |
| `gui:bundle` | macOS `DBCrust.app` bundle |
| `gui:test` | Backend tests (`dbcrust-gui-core`) |
| `gui:lint` | Clippy on both GUI crates |
| `gui:screenshots` | Render docs screenshots headlessly |
| `gui:clean` | Remove GUI build artifacts |
| `cache:stats` | Build-cache savings and disk use (`mbx stats`) |
| `cache:gc` | Reclaim build-cache disk space (`mbx gc`) |
| `py:dev` | `maturin develop` |
| `py:build` | `maturin build --release` |
| `py:test` | `python -m pytest` |
| `all:build` | Build CLI + GUI + Python |
| `all:clean` | Clean everything |

## Project layout

```
├── src/                       # `dbcrust` (root crate): CLI, REPL, config, PyO3 bindings
│   ├── main.rs                # tokio entry point
│   ├── lib.rs                 # public API (re-exports the crates below under their old paths) + PyO3 bindings
│   ├── cli.rs                 # argument parsing (usage-rs derive)
│   ├── cli_core.rs            # REPL loop, one-shot mode and command dispatch
│   ├── commands.rs            # backslash command enum (strum-driven)
│   ├── config.rs              # TOML config + session + named query storage
│   ├── config_editor.rs       # schema-driven `\config` menu / get / set
│   ├── db.rs                  # `Database` session: connect flow, docker/SSH tunnels, EXPLAIN
│   ├── drivers.rs             # facade over dbcrust-drivers
│   ├── ai_session_impl.rs     # `impl AiSession for Database` (the assistant lives in dbcrust-ai)
│   ├── completion.rs          # SQL autocompletion engine
│   ├── command_completion.rs  # backslash command completion
│   ├── shell_completion.rs    # shell completion generation (bash/zsh/fish/powershell)
│   ├── prompt.rs              # reedline custom prompt
│   ├── highlighter.rs         # SQL syntax highlighting
│   ├── sql_validator.rs       # reedline validator (Enter submits or continues)
│   ├── sql_parser_*.rs        # per-engine parsers used by completion
│   ├── url_scheme.rs          # URL scheme parsing and normalization
│   ├── ssh_tunnel.rs          # SSH tunnel management
│   ├── vault_client.rs        # Vault HTTP client
│   ├── named_queries.rs       # named query parameter substitution
│   ├── pgpass.rs, myconf.rs   # .pgpass / .my.cnf support
│   ├── history_manager.rs     # per-session history
│   ├── script.rs, pager.rs    # external editor integration, output paging
│   ├── logging.rs             # tracing setup
│   ├── update.rs              # `--update`
│   ├── secrets_cli/           # `dbcrust secrets` status / audit / migrate
│   ├── broker/                # `dbcrust broker …` front end (prompts, arguments, e2e tests)
│   ├── features.rs            # `FeatureNotCompiled`: the error of an add-on the build leaves out
│   ├── explain_tui/           # interactive EXPLAIN TUI (ratatui + crossterm; feature `explain-tui`)
│   └── schema_tui/            # interactive schema browser TUI (feature `explain-tui`)
├── crates/
│   ├── core/                  # dbcrust-core: client traits, model DTOs, SQL/format/display, safety, password sanitizer
│   ├── secrets/               # dbcrust-secrets: keychain / Argon2id file store, legacy readers, config-dir/test-mode paths
│   ├── drivers/               # dbcrust-drivers: engine registry, `create_database_client`, one cargo feature per engine
│   ├── driver-postgres/       # one crate per engine: -mysql, -sqlite, -mongodb, -clickhouse,
│   │                          #   -elasticsearch, -white-dragon, -files (DataFusion: Parquet/CSV/JSON)
│   ├── docker/                # dbcrust-docker: daemon selection, discovery, route probing, `docker exec` relay (bollard)
│   ├── ai/                    # dbcrust-ai: genai assistant, agent loop, ChatGPT sign-in, `AiSession` trait
│   ├── broker/                # dbcrust-broker: credential broker core (registry, admin, gateway, audit, socket server)
│   ├── agent-protocol/        # wire protocol shared by the broker and the thin agent client
│   └── agent/                 # dbcrust-agent: credential-free client for AI coding agents
├── gui/                       # desktop app (GPUI Kit)
│   ├── core/                  # dbcrust-gui-core: workers, storage, models (no GPUI)
│   ├── src/                   # dbcrust-gui: views, workspace, commands
│   └── ARCHITECTURE.md        # module map and design language
├── python/                    # Python package (PyO3 + maturin)
├── docs/                      # MkDocs Material documentation
├── mise.toml                  # tool + task definitions
├── bin/mise                   # mise wrapper: runs a pinned mise without installing it (bin/mise.cmd on Windows)
├── Cargo.toml                 # workspace root
├── pyproject.toml             # Python package metadata
└── mkdocs.yml                 # docs site config
```

## Architecture notes

- **Command system**: All backslash commands are variants of a `Command` enum. The `CommandShortcut` enum (with `strum::EnumIter`) auto-generates help text, completion, and dispatch. Never use hardcoded arrays for command lists.
- **Database abstraction**: The `DatabaseClient` and `MetadataProvider` traits in `database.rs` define the interface. Each database has its own implementation file.
- **Async**: Database operations use `tokio` + `async-trait`. Shared state uses `Arc<Mutex<T>>`.
- **Desktop app**: `gui/core` (`dbcrust-gui-core`) runs each connection on its own thread with a current-thread runtime + `LocalSet` (DBCrust futures are `!Send` and sqlx pools must stay on their runtime); the GPUI views in `gui/src` await its `Send` futures. Cancellation is per request via `dbcrust::database::set_thread_interrupt_flag`.
- **Error handling**: `thiserror` for custom error types. User-facing prompts use `inquire`.
- **Config**: `serde` with `#[serde(default)]` for backward compatibility. Separate TOML files for settings, sessions, named queries, and recent connections.

## Conventional commits

The project uses [Commitizen](https://commitizen-tools.github.io/commitizen/) for conventional commits:

```bash
cz commit    # interactive commit
cz bump      # bump version based on commit history
```

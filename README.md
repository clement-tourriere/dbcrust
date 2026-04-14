# DBCrust

**A fast, multi-database CLI and desktop app built in Rust.** Connect to PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB, and Elasticsearch — or query Parquet, CSV, and JSON files with SQL. Includes SSH tunneling, Vault integration, Docker auto-discovery, and a Tauri-based GUI.

[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Documentation](https://img.shields.io/badge/docs-mkdocs-blue.svg)](https://clement-tourriere.github.io/dbcrust/)
[![PyPI](https://img.shields.io/pypi/v/dbcrust.svg)](https://pypi.org/project/dbcrust/)

## Features

| Category | What you get |
|----------|-------------|
| **Databases** | PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB, Elasticsearch |
| **File formats** | Parquet, CSV, JSON — queried with SQL via Apache DataFusion |
| **Smart CLI** | Context-aware autocompletion, syntax highlighting, history search, external editor |
| **Desktop GUI** | Tauri app with CodeMirror SQL editor, schema explorer, Docker panel, EXPLAIN viewer |
| **Security** | SSH tunneling, HashiCorp Vault dynamic credentials, encrypted password storage |
| **DevOps** | Docker container auto-discovery, saved sessions, recent connections |
| **Performance** | EXPLAIN visualization (text + interactive TUI), query timing |
| **Django** | ORM analyzer, N+1 detection, middleware, management commands |
| **Python API** | `dbcrust.run_command()`, `dbcrust.run_cli()`, `PostgresClient` class |

## Install

```bash
# Pre-built binary (macOS / Linux)
curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh

# Windows
# irm https://clement-tourriere.github.io/dbcrust/install.ps1 | iex

# Python (via uv)
uv tool install dbcrust        # install globally
uvx dbcrust <connection-url>   # run without installing

# From source
cargo install --path .
```

## Connect

```bash
# Relational databases
dbcrust postgres://user:pass@localhost/mydb
dbcrust mysql://root:pass@localhost:3306/mydb
dbcrust sqlite:///path/to/db.sqlite
dbcrust clickhouse://user:pass@localhost:8123/default

# Document databases
dbcrust mongodb://user:pass@localhost:27017/mydb
dbcrust elasticsearch://localhost:9200

# File formats (SQL via DataFusion)
dbcrust parquet:///data/sales.parquet
dbcrust csv:///logs/*.csv?header=true
dbcrust json:///events.json

# Docker auto-discovery
dbcrust docker://                         # interactive picker
dbcrust docker://my-postgres-container    # direct

# Saved sessions & recent connections
dbcrust session://production_db
dbcrust recent://

# Vault dynamic credentials
dbcrust vault://readonly@database/postgres-prod

# SSH tunneling
dbcrust postgres://user@db.internal/app --ssh-tunnel jumphost.com
```

Both `dbcrust` and `dbc` (short alias) are available.

## Interactive commands

Once connected you get a REPL with 50+ backslash commands. Highlights:

```
\dt             list tables               \l        list databases
\d <table>      describe table            \c <db>   switch database
\e              toggle EXPLAIN mode       \ev       EXPLAIN TUI (interactive)
\x              expanded display          \cs       column selection
\ed             open $EDITOR              \w <f>    write last query to file
\i <f>          execute SQL file          \n        list named queries
\ns <n> <sql>   save named query          \ss <n>   save session
\s              list sessions             \r        recent connections
\savepass       save password             \vc       Vault cache status
\docker         list Docker containers    \h        help
\q              quit
```

Named queries support parameter substitution (`$1`, `$*`, `$@`) and scopes (`--global`, `--postgres`, `--mysql`, `--sqlite`, or session-local by default).

## Desktop GUI

DBCrust includes a Tauri-based desktop application with:

- **SQL editor** — CodeMirror with syntax highlighting, `Cmd+Enter` / `Ctrl+Enter` to run
- **EXPLAIN viewer** — visual query plan display
- **Schema explorer** — browse tables, columns, indexes, foreign keys
- **Docker discovery** — find and connect to running database containers
- **Session manager** — saved connections, recent history
- **Multi-tab** — work on multiple queries in parallel
- **Settings** — view and toggle configuration
- **System tray** — quick access, stays running in background

### Run the GUI

```bash
# Prerequisites: mise (installs Bun automatically)
mise install

# Development mode (hot-reload)
mise run gui:dev

# Production build (generates .app / .dmg / .msi)
mise run gui:build
```

## Python integration

```python
import dbcrust

# Execute a query
result = dbcrust.run_command("postgres://user:pass@localhost/mydb", "SELECT * FROM users LIMIT 5")

# Launch interactive CLI
dbcrust.run_cli("postgres://user:pass@localhost/mydb")

# Object-oriented client
from dbcrust import PostgresClient
client = PostgresClient(host="localhost", user="postgres", dbname="myapp")
tables = client.list_tables()
```

### Django ORM analyzer

```python
# settings.py — add the middleware
MIDDLEWARE = ['dbcrust.django.PerformanceAnalysisMiddleware', ...]

# Or analyze manually
from dbcrust.django import analyzer
with analyzer.analyze() as analysis:
    for book in Book.objects.all():
        print(book.author.name)  # N+1 detected

results = analysis.get_results()
```

```bash
# Connect using Django database settings
python manage.py dbcrust
```

## Development

DBCrust uses **[mise](https://mise.jdx.dev/)** for tool management and task running. Mise automatically installs **Bun** (used for the GUI frontend), **commitizen**, and other dev tools.

```bash
# One-time setup
mise install              # installs Bun, commitizen, pkl, etc.
mise run gui:install      # install GUI npm dependencies via Bun

# CLI
mise run build:dev        # debug build
mise run build            # release build
cargo run -- <url>        # run directly

# GUI
mise run gui:dev          # dev mode with hot-reload
mise run gui:build        # production build

# Python
mise run py:dev           # maturin develop
mise run py:build         # build wheel
mise run py:test          # pytest

# Quality
mise run fmt              # cargo fmt
mise run lint             # clippy
mise run test             # cargo test
mise run check            # fmt + lint + test
```

### Project layout

```
├── src/                   # Rust CLI + library
│   ├── main.rs            # entry point
│   ├── commands.rs        # backslash command system (enum + strum)
│   ├── database_*.rs      # per-database implementations
│   ├── completion.rs      # SQL autocompletion
│   ├── explain_tui/       # interactive EXPLAIN visualizer (ratatui)
│   └── ...
├── gui/                   # Tauri desktop app
│   ├── src/               # React + TypeScript frontend
│   ├── src-tauri/         # Tauri Rust backend (bridges to dbcrust core)
│   └── package.json       # Bun-managed dependencies
├── python/                # Python bindings (PyO3 + maturin)
├── docs/                  # MkDocs documentation source
├── mise.toml              # task runner & tool config
└── Cargo.toml             # workspace root
```

## Configuration

Config lives in `~/.config/dbcrust/`:

```
config.toml               # settings (limits, display, SSH patterns, Vault, etc.)
named_queries.toml         # saved queries with scopes
recent.toml                # connection history
vault_credentials.enc      # encrypted Vault credential cache
history.txt                # command history
```

Show current config: `\config` inside the REPL.

## Documentation

- **[Full docs](https://clement-tourriere.github.io/dbcrust/)** — installation, user guide, reference
- **[Quick start](https://clement-tourriere.github.io/dbcrust/quick-start/)** — get connected in 2 minutes
- **[Command reference](https://clement-tourriere.github.io/dbcrust/reference/backslash-commands/)** — all 50+ commands
- **[Django integration](https://clement-tourriere.github.io/dbcrust/django-analyzer/)** — ORM analysis
- **[Python API](https://clement-tourriere.github.io/dbcrust/python-api/overview/)** — programmatic usage

## License

MIT — see [LICENSE](LICENSE).

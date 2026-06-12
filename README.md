# DBCrust

**One fast client for every database.** PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB, Elasticsearch — and SQL over Parquet, CSV, and JSON files. Built in Rust, with an AI assistant, smart autocompletion, SSH tunneling, Vault integration, and a desktop GUI.

[![CI](https://github.com/clement-tourriere/dbcrust/actions/workflows/ci.yml/badge.svg)](https://github.com/clement-tourriere/dbcrust/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/dbcrust.svg)](https://pypi.org/project/dbcrust/)
[![Documentation](https://img.shields.io/badge/docs-website-blue.svg)](https://clement-tourriere.github.io/dbcrust/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

```bash
curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh
dbcrust postgres://user:pass@localhost/mydb
```

[Documentation](https://clement-tourriere.github.io/dbcrust/) · [Quick start](https://clement-tourriere.github.io/dbcrust/quick-start/) · [Command reference](https://clement-tourriere.github.io/dbcrust/reference/backslash-commands/) · [Python API](https://clement-tourriere.github.io/dbcrust/python-api/overview/)

## Why DBCrust

- **Every database, one tool** — the same REPL, commands, and muscle memory across PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB, and Elasticsearch.
- **AI assistant built in** — type `?? top 10 customers by revenue` and get SQL generated from your actual schema, shown before it runs. Works with Anthropic, OpenAI, Gemini, Ollama, and 20+ other providers.
- **Files are databases too** — run SQL directly on Parquet, CSV, and JSON via Apache DataFusion.
- **Production-friendly plumbing** — SSH tunnels (with auto-tunnel patterns), HashiCorp Vault dynamic credentials, Docker container auto-discovery, encrypted password storage.
- **A REPL that helps** — context-aware autocompletion, syntax highlighting, history search, external editor, EXPLAIN visualization (including an interactive TUI), named queries, saved sessions.
- **Scriptable and embeddable** — `-c` for one-shot queries, a Python API powered by the same Rust core, and a Django ORM performance analyzer.

## Install

```bash
# Pre-built binary — macOS & Linux
curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh

# Pre-built binary — Windows (PowerShell)
irm https://clement-tourriere.github.io/dbcrust/install.ps1 | iex

# Python 3.10+ (ships the same native binary)
uv tool install dbcrust          # or: pipx install dbcrust / pip install dbcrust
uvx dbcrust <url>                # run without installing

# From source (Rust 1.85+)
cargo install --path .
```

Two binaries are installed: `dbcrust` and the short alias `dbc`.

```bash
dbcrust --update                 # self-update (detects uv / pipx / pip / cargo / binary installs)
dbcrust --completions zsh        # shell completions (bash, zsh, fish, powershell, ...)
```

## Quick start

```bash
dbcrust postgres://user:pass@localhost/mydb     # interactive session
dbcrust recent://                               # pick from recent connections
dbcrust sqlite:///path/to/db.sqlite -c "SELECT count(*) FROM users"   # run and exit
```

Every connection type is a URL:

| Scheme | Example |
|--------|---------|
| PostgreSQL | `postgres://user:pass@localhost:5432/mydb?sslmode=require` |
| MySQL | `mysql://root:pass@localhost:3306/mydb` |
| SQLite | `sqlite:///path/to/db.sqlite` |
| ClickHouse | `clickhouse://user:pass@localhost:8123/default` |
| MongoDB | `mongodb://user:pass@localhost:27017/mydb` |
| Elasticsearch | `elasticsearch://localhost:9200` |
| Parquet / CSV / JSON | `parquet:///data/*.parquet` · `csv:///logs/app.csv?header=true` · `json:///events.json` |
| Docker container | `docker://` (interactive picker) · `docker://my-postgres/mydb` |
| Saved session | `session://production_db` |
| Recent connections | `recent://` |
| Vault credentials | `vault://readonly@database/postgres-prod` |

Full details: [URL schemes reference](https://clement-tourriere.github.io/dbcrust/reference/url-schemes/).

## The interactive session

Connecting drops you into a REPL with context-aware SQL autocompletion, syntax highlighting, searchable history, and 60+ psql-style backslash commands. The most used:

| | Commands |
|---|---|
| **Explore** | `\l` databases · `\c <db>` switch · `\dt` tables · `\d <table>` describe |
| **Display** | `\x` expanded · `\cs` column selection · `\e` EXPLAIN mode · `\ev` interactive EXPLAIN TUI |
| **Edit & run** | `\ed` open `$EDITOR` · `\i <file>` run SQL file · `\w <file>` write last query |
| **Named queries** | `\n` list · `\ns <name> <sql>` save · `\nd <name>` delete |
| **Connections** | `\ss <name>` save session · `\s` list sessions · `\r` recent · `\docker` containers |
| **Credentials** | `\savepass` store password · `\vc` Vault cache status |
| **Meta** | `\ai` AI assistant · `\config` settings editor · `\h` help · `\q` quit |

Named queries support positional parameters (`$1`, `$*`, `$@`) and scopes — global, per-database-type, or session-local.

See the [full command reference](https://clement-tourriere.github.io/dbcrust/reference/backslash-commands/).

## AI assistant

Turn natural language into SQL without leaving your session. The assistant uses your database's real schema as context, streams its answer, and **always shows the SQL before running it** (writes default to "No").

```sql
\ai setup                                        -- one-time wizard: provider, model, API key

?? top 10 customers by total order value this year
?? now only the active ones                      -- follow-ups keep conversation context
```

- **Providers**: Anthropic, OpenAI, Gemini, Ollama, Groq, DeepSeek, xAI, OpenRouter, and more — 25+ via [genai](https://crates.io/crates/genai), including any OpenAI-compatible endpoint for self-hosted models.
- **Private by design**: disabled by default; only schema metadata and your question are sent — never row data. API keys live in your OS keychain, an encrypted file, or environment variables.

More in the [AI assistant guide](https://clement-tourriere.github.io/dbcrust/user-guide/ai-assistant/).

## Tunnels, Vault & Docker

**SSH tunneling** — reach databases behind a jump host, explicitly or automatically via config patterns:

```bash
dbcrust postgres://user@db.internal/app --ssh-tunnel jumphost.example.com
```

```toml
# ~/.config/dbcrust/config.toml — auto-tunnel any host matching the pattern
[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "user@jumphost.example.com:2222"
```

**HashiCorp Vault** — dynamic database credentials with an encrypted local cache: `dbcrust vault://readonly@database/postgres-prod`.

**Docker** — `dbcrust docker://` lists running database containers and connects without you hunting for ports or credentials.

## Python & Django

The Python package wraps the same Rust core via PyO3 — identical URLs, commands, and behavior.

```python
import dbcrust

dbcrust.run_command("postgres://user:pass@localhost/mydb", "SELECT * FROM users LIMIT 5")
dbcrust.run_cli("postgres://user:pass@localhost/mydb")     # full interactive REPL

from dbcrust import PostgresClient
client = PostgresClient(host="localhost", user="postgres", dbname="myapp")
tables = client.list_tables()
```

The Django integration detects N+1 queries and other ORM performance issues:

```python
# settings.py
MIDDLEWARE = ["dbcrust.django.PerformanceAnalysisMiddleware", ...]

# or analyze a block of code explicitly
from dbcrust.django import analyzer
with analyzer.analyze() as analysis:
    for book in Book.objects.all():
        print(book.author.name)        # N+1 detected
results = analysis.get_results()
```

```bash
python manage.py dbcrust               # connect using your Django DB settings
```

Guides: [Python API](https://clement-tourriere.github.io/dbcrust/python-api/overview/) · [Django analyzer](https://clement-tourriere.github.io/dbcrust/django-analyzer/).

## Desktop GUI

A Tauri-based desktop app ships in the repo (built from source for now): CodeMirror SQL editor, schema explorer, visual EXPLAIN viewer, Docker discovery, multi-tab queries, and a system tray. See the [GUI guide](https://clement-tourriere.github.io/dbcrust/user-guide/gui/).

```bash
mise install && mise run gui:install
mise run gui:dev                       # development (hot-reload)
mise run gui:build                     # production .app / .dmg / .msi
```

## Configuration

Settings live in `~/.config/dbcrust/`, with user data kept in dedicated files:

| File | Contents |
|------|----------|
| `config.toml` | App settings (display, limits, SSH patterns, AI, logging, ...) |
| `sessions.toml` | Saved sessions |
| `recent.toml` | Recent connections |
| `named_queries.toml` | Named queries |
| `vault_credentials.enc` | Encrypted Vault credential cache |
| `~/.dbcrust` | Stored passwords (pgpass-style) |

Edit configuration interactively or from scripts — no connection required:

```bash
dbcrust config                         # interactive menu (also \config inside the REPL)
dbcrust config get logging.level
dbcrust config set logging.level debug
dbcrust config edit                    # open config.toml in $EDITOR
```

Full list of options: [configuration reference](https://clement-tourriere.github.io/dbcrust/reference/configuration-reference/).

## Development

DBCrust uses [mise](https://mise.jdx.dev/) for toolchain and task management — `mise install` sets up everything (Bun for the GUI, commitizen, etc.).

```bash
mise run build:dev        # debug build          mise run build      # release build
mise run test             # cargo test           mise run check      # fmt + lint + test
mise run py:dev           # maturin develop      mise run py:test    # Python tests
mise run gui:dev          # GUI with hot-reload  mise run docs       # docs dev server
```

```
src/                Rust core — CLI, REPL, database backends, AI assistant
├── commands.rs     backslash command system (enum + strum)
├── database_*.rs   per-database implementations
└── explain_tui/    interactive EXPLAIN visualizer (ratatui)
gui/                Tauri desktop app (React + TypeScript, Bun)
python/             Python bindings (PyO3 + maturin) and Django integration
docs/               documentation site (Astro Starlight)
```

More in the [development guide](https://clement-tourriere.github.io/dbcrust/user-guide/development/).

## License

MIT — see [LICENSE](LICENSE).

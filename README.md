# DBCrust

**A fast psql-style database workbench for your terminal.** One CLI for PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB, Elasticsearch, Docker databases, Vault-backed connections, and SQL over Parquet/CSV/JSON files — with optional AI, Django ORM analysis, Python bindings, and a desktop GUI.

[![CI](https://github.com/clement-tourriere/dbcrust/actions/workflows/ci.yml/badge.svg)](https://github.com/clement-tourriere/dbcrust/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/dbcrust.svg)](https://pypi.org/project/dbcrust/)
[![Documentation](https://img.shields.io/badge/docs-website-blue.svg)](https://clement-tourriere.github.io/dbcrust/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

```bash
curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh
dbc postgres://user:pass@localhost/mydb
```

> **AI is optional and disabled by default.** DBCrust works with zero AI setup. `??` sends schema metadata and your question, not row data; `???` and Django AI investigations can inspect bounded query results. Generated SQL is shown before execution.

[Documentation](https://clement-tourriere.github.io/dbcrust/) · [Quick start](https://clement-tourriere.github.io/dbcrust/quick-start/) · [AI/privacy](https://clement-tourriere.github.io/dbcrust/user-guide/ai-assistant/#privacy-notes) · [Django analyzer](https://clement-tourriere.github.io/dbcrust/django-analyzer/) · [Python API](https://clement-tourriere.github.io/dbcrust/python-api/overview/)

## Why DBCrust

- **One workflow across databases** — the same REPL, commands, and muscle memory across PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB, and Elasticsearch.
- **Files are databases too** — inspect Parquet, CSV, and JSON with SQL via Apache DataFusion, no import step or notebook required.
- **Optional AI you control** — type `?? top 10 customers by revenue` to generate SQL from schema context, or use `???` for bounded read-only investigations. Supports Anthropic, OpenAI, Gemini, Ollama, and 20+ other providers.
- **DBCrust for Django** — catch N+1 queries, missing `select_related` / `prefetch_related`, slow views, and index opportunities before production.
- **Production-friendly plumbing** — SSH tunnels (with auto-tunnel patterns), HashiCorp Vault dynamic credentials, Docker container auto-discovery, encrypted password storage.
- **A REPL that helps** — context-aware autocompletion, syntax highlighting, history search, external editor, EXPLAIN visualization (including an interactive TUI), named queries, saved sessions.
- **Scriptable and embeddable** — `-c` for one-shot queries and a Python API powered by the same Rust core.

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
dbc ./users.csv                                 # infer CSV from the extension
dbcrust sqlite:///path/to/db.sqlite -c "SELECT count(*) FROM users"   # run and exit
```

Every connection type is a URL:

| Scheme | Example |
|--------|---------|
| PostgreSQL | `postgres://user:pass@localhost:5432/mydb?sslmode=require` |
| MySQL | `mysql://root:pass@localhost:3306/mydb` |
| SQLite | `sqlite:///path/to/db.sqlite` or `./path/to/db.sqlite` |
| ClickHouse | `clickhouse://user:pass@localhost:8123/default` |
| MongoDB | `mongodb://user:pass@localhost:27017/mydb` |
| Elasticsearch | `elasticsearch://localhost:9200` |
| Parquet / CSV / JSON | `./data.parquet` · `./logs/app.csv` · `file://` picker · `json:///events.json` |
| Docker container | `docker://` (interactive picker) · `docker://my-postgres/mydb` |
| Saved session | `session://production_db` |
| Recent connections | `recent://` |
| Vault credentials | `vault://readonly@database/postgres-prod` |

Full details: [URL schemes reference](https://clement-tourriere.github.io/dbcrust/reference/url-schemes/).

## SQL over local files

Inspect production exports, logs, and data drops without importing them into a database or opening a notebook.

```bash
dbc warehouse/events.parquet      # inferred from extension
dbc 'logs/*.csv?header=true'      # globs work too
dbc file://                       # interactive compatible-file picker
dbc json:///tmp/api-responses.ndjson
```

```sql
SELECT date_trunc('hour', ts) AS hour, count(*)
FROM events
WHERE level = 'ERROR'
GROUP BY hour
ORDER BY hour DESC;
```

DBCrust registers matching files as SQL tables and lets DataFusion handle filtering, aggregations, joins, nested JSON fields, and glob patterns. See the [file formats guide](https://clement-tourriere.github.io/dbcrust/user-guide/file-formats/).

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
- **Privacy controls**: AI is opt-in. `??` sends schema metadata and your prompt/history; query results stay local. `???` and Django "Investigate with AI" can send bounded result rows, query plans, captured SQL, and source context. API keys live in your OS keychain, an encrypted file, or environment variables.

More in the [AI assistant guide](https://clement-tourriere.github.io/dbcrust/user-guide/ai-assistant/) and [privacy notes](https://clement-tourriere.github.io/dbcrust/user-guide/ai-assistant/#privacy-notes).

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

## Python API & DBCrust for Django

The Python package wraps the same Rust core via PyO3 — identical URLs, commands, and behavior.

```python
import dbcrust

dbcrust.run_command("postgres://user:pass@localhost/mydb", "SELECT * FROM users LIMIT 5")
dbcrust.run_cli("postgres://user:pass@localhost/mydb")     # full interactive REPL

from dbcrust import PostgresClient
client = PostgresClient(host="localhost", user="postgres", dbname="myapp")
tables = client.list_tables()
```

DBCrust for Django catches ORM performance bugs before production: N+1 queries, duplicate queries, missing `select_related` / `prefetch_related`, slow views, and index opportunities, with recommendations tied back to code locations.

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

Settings live in `~/.config/dbcrust/` by default (override with `DBCRUST_CONFIG_DIR=/path/to/dbcrust-config-dir`), with user data kept in dedicated files:

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

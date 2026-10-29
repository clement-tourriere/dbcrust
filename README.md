# DBCrust

**A fast psql-style database workbench for your terminal.** One CLI for PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB, Elasticsearch, White Dragon search, Redis/Valkey, SQL Server/Azure SQL, Amazon Redshift, libSQL/Turso, InfluxDB, Amazon S3/R2/MinIO object stores, DynamoDB and CloudWatch Logs (opt-in AWS builds), DuckDB/DuckLake (opt-in build), Docker databases, Vault-backed connections, and SQL over Parquet/CSV/JSON files — with optional AI, Django ORM analysis, Python bindings, and a desktop GUI.

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
- **Agent-safe connections** — a credential broker (`dbcrust broker`, `dbcrust-agent`, MCP) keeps database passwords out of AI agents' reach: enrolled aliases, forced read-only, row/time limits and an audit log; it can run as its own OS user. See [Agent connections](https://clement-tourriere.github.io/dbcrust/user-guide/agent-connections/).
- **Scriptable, embeddable, agent-ready** — `-c`/`-f`/stdin one-shots with `-o json|csv` output, stable exit codes, a `--read-only` guard, and a Python API powered by the same Rust core.

## Install

```bash
# Pre-built binary — macOS & Linux
curl -fsSL https://clement-tourriere.github.io/dbcrust/install.sh | sh

# Pre-built binary — Windows (PowerShell)
irm https://clement-tourriere.github.io/dbcrust/install.ps1 | iex

# Homebrew — macOS & Linux
brew install clement-tourriere/tap/dbcrust

# Python 3.10+ (ships the same native binary)
uv tool install dbcrust          # or: pipx install dbcrust / pip install dbcrust
uvx dbcrust <url>                # run without installing

# From source (Rust 1.85+)
cargo install --path .
```

Two binaries are installed: `dbcrust` and the short alias `dbc`.

```bash
dbcrust --update                 # self-update (detects uv / pipx / pip / Homebrew / cargo / binary installs)
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
| White Dragon | `white-dragon://localhost:7700` |
| Redis / Valkey | `redis://localhost:6379/0` · `rediss://user:pass@host:6380` (TLS) |
| SQL Server / Azure SQL | `mssql://user:pass@localhost:1433/mydb` (TLS verified by default) · `mssql://user:pass@host%5CINSTANCE/mydb` |
| Amazon Redshift | `redshift://user:pass@cluster.abc.eu-west-1.redshift.amazonaws.com:5439/dev` (TLS required by default) |
| libSQL / Turso | `libsql://my-db-org.turso.io?authToken=…` (HTTPS) · `libsql+http://localhost:8080` (local sqld) |
| InfluxDB | `influxdb://localhost:8086/db` (v1, InfluxQL) · `influxdb2://:TOKEN@localhost/bucket?org=acme` (v2, Flux) · `influxdb3://:TOKEN@localhost/db` (v3, SQL) · `influxdbs://…` (TLS) |
| Amazon DynamoDB *(opt-in: `--features aws`, Rust ≥ 1.94.1)* | `dynamodb://eu-west-1/Orders?profile=prod` · `dynamodb+http://localhost:8000?region=local` — PartiQL `SELECT` and `tables`, `describe`, `scan`, `get`, `query`; bounded reads, capacity reported, no writes |
| Amazon CloudWatch Logs *(opt-in: `--features aws`, Rust ≥ 1.94.1)* | `cloudwatch://eu-west-1/aws/lambda/orders?profile=prod` — Logs Insights over a mandatory time window (`since 30m fields @message \| filter …`), `groups`, `streams`, `tail`; queries are stopped on Ctrl-C and timeout, scanned bytes reported |
| Amazon S3 / R2 / MinIO *(opt-in: `--features aws`, Rust ≥ 1.94.1)* | `s3://my-bucket/logs/?region=eu-west-1` · `s3://KEY:SECRET@bucket?endpoint=https://host:9000` · `r2://account/bucket` · `minio://host:9000/bucket` — typed read-only commands: `ls`, `head`, `get`, `buckets` |
| DuckDB / DuckLake (opt-in: `--features duckdb`) | `duckdb:///data/warehouse.duckdb` · `duckdb://:memory:` · `warehouse.duckdb` · `--read-only` is enforced by the engine |
| Parquet / CSV / JSON | `./data.parquet` · `./logs/app.csv` · `file://` picker · `json:///events.json` |
| Docker container | `docker://` (interactive picker) · `docker://my-postgres/mydb` |
| Saved session | `session://production_db` |
| Recent connections | `recent://` |
| Vault credentials | `vault://readonly@database/postgres-prod` |

Full details: [URL schemes reference](https://clement-tourriere.github.io/dbcrust/reference/url-schemes/).

## Built for AI agents

Coding agents work best with CLIs they can shell out to — no MCP server to run, no per-tool token overhead, composable with pipes and exit codes. DBCrust's one-shot mode gives them **one binary for every data service**:

```bash
dbcrust session://prod -c '\ddl'                            # whole schema as compact DDL, one call
dbcrust session://prod --read-only --no-input -o json \
  -c "SELECT status, count(*) FROM orders GROUP BY status"  # single-line {"columns","rows",...} envelope
echo "SELECT count(*) FROM logs" | dbcrust ./logs.parquet   # stdin scripts; files are databases too
```

The contract is built for programmatic callers: results-only stdout (status goes to stderr, the pager never engages), JSON errors on stderr under `-o json`, stable exit codes (`0` ok · `1` SQL error · `2` usage · `3` connection · `4` blocked by `--read-only`), `--timeout`/`--max-rows` limits, and `--no-input` so no prompt can ever hang an agent. The binary documents itself: run **`dbcrust agents`** for the full ~130-line contract, and paste the ready-made snippet into your project's `CLAUDE.md`/`AGENTS.md` from the [agent quickstart](https://clement-tourriere.github.io/dbcrust/ai-agents/quickstart/).

`--read-only` is a best-effort statement guard backed by connect-level hardening (SQLite `query_only`, PostgreSQL `default_transaction_read_only`); for hard guarantees use a read-only database role — details in [safety & guardrails](https://clement-tourriere.github.io/dbcrust/ai-agents/safety/).

For databases that matter, don't hand the agent a password at all: enroll the connection with the **credential broker** (`sudo dbcrust broker install-service`, then `dbcrust broker enroll …`) and give the agent the credential-free `dbcrust-agent` client (CLI or MCP server). See [Agent connections](https://clement-tourriere.github.io/dbcrust/user-guide/agent-connections/).

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
| **Explore** | `\l` databases · `\c <db>` switch · `\dt` tables · `\d <table>` describe · `\ddl` schema dump |
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
- **Privacy controls**: AI is opt-in. `??` sends schema metadata and your prompt/history; query results stay local. `???` and Django "Investigate with AI" can send bounded result rows, query plans, captured SQL, and source context. API keys live in the secret store (OS keychain, or a passphrase-encrypted file on Linux) or environment variables.

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

**HashiCorp Vault** — dynamic database credentials with an encrypted local cache: `dbcrust vault://readonly@database/postgres-prod`. An optional AWS-style `vault_credential_process` can obtain authentication lazily, so ordinary PostgreSQL/MySQL connections never start a Vault login flow. The helper is provider-neutral: any command that prints a token or JSON `{ "token": "…", "vault_addr": "…" }` works.

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

A native, GPU-accelerated desktop app built with [GPUI Kit](https://gpui-kit.com), pure Rust on the same core as the CLI. On macOS, download the signed and notarized [DBCrust-macos-universal.dmg](https://github.com/clement-tourriere/dbcrust/releases/latest/download/DBCrust-macos-universal.dmg) or run `brew install --cask clement-tourriere/tap/dbcrust-gui`; on Linux and Windows, build it from source. See the [GUI guide](https://clement-tourriere.github.io/dbcrust/user-guide/gui/).

![DBCrust desktop app](docs/public/screenshots/gui-query.png)

- **Connect:** one searchable list of saved connections, recent ones and Docker containers, with Test / Save / Connect, environment labels and a Vault wizard. Docker containers connect even when their port isn't published or a VPN blocks the container network: DBCrust relays through Docker itself. Open several connections at once, read-only if you like.
- **Explore:** a schema explorer with a cross-connection filter, and a table inspector showing columns, indexes, constraints, references, a data preview and DDL.
- **Edit:** a SQL editor with schema-aware completion (aliases and CTEs resolved). Run the statement under the caret, run all, really cancel, format, keep history, and use named queries and `.sql` files.
- **Results:** a virtualized results grid with distinct NULLs and numeric sorting, copy/export as CSV, TSV, JSON or Markdown, and a row inspector.
- **Plans:** an EXPLAIN visualizer with plan tree, cost shares, row-estimate checks and recommendations.
- **Navigate:** a command palette (`⌘K`), native menus, full keyboard shortcuts, light/dark themes following the OS plus bundled themes, and drafts that are autosaved and restored.
- **Stays at hand:** closing the window keeps DBCrust in the menu bar / system tray with its connections and running queries; reopen or connect from there.

<p>
  <img src="docs/public/screenshots/gui-home.png" alt="Connect page" width="49%">
  <img src="docs/public/screenshots/gui-explain.png" alt="EXPLAIN visualizer" width="49%">
</p>

```bash
mise run gui:dev                       # run the app
mise run gui:build                     # optimised build (target/gui-release/dbcrust-gui)
mise run gui:bundle                    # macOS DBCrust.app
mise run macos:package                 # macOS release packages: CLI archive + DMG
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

DBCrust uses [mise](https://mise.jdx.dev/) 2026.8.16+ for toolchain and task management. `mise install` installs Bun (docs site), Commitizen, and [Mr Boxington](https://mr-boxington.jdx.dev/), which puts a shared, self-pruning cache behind Cargo commands run through mise. No mise? Run `./bin/mise` instead of `mise`: the committed wrapper downloads a pinned mise on first use.

```bash
mise install
mise run build:dev        # debug build          mise run build      # release build
mise run test             # cargo test           mise run check      # fmt + lint + test
mise run py:dev           # maturin develop      mise run py:test    # Python tests
mise run gui:dev          # desktop app         mise run docs       # docs dev server
mise run cache:stats      # mbx cache savings    mbx doctor          # verify the Rust build cache
```

With mise activated, plain `cargo` commands use the same cache. Set `MBX_DISABLE=1` for a one-off uncached Cargo command.

```
src/                Rust core — CLI, REPL, database backends, AI assistant
├── commands.rs     backslash command system (enum + strum)
├── database_*.rs   per-database implementations
└── explain_tui/    interactive EXPLAIN visualizer (ratatui)
gui/                desktop app (GPUI Kit) — core/ is the GPUI-free backend
python/             Python bindings (PyO3 + maturin) and Django integration
docs/               documentation site (Astro Starlight)
```

More in the [development guide](https://clement-tourriere.github.io/dbcrust/user-guide/development/).

## License

MIT — see [LICENSE](LICENSE).

# DBCrust for AI agents

One binary that talks to PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB,
Elasticsearch, and data files (Parquet/CSV/JSON) — with a one-shot mode built
for programmatic callers: structured output, stable exit codes, and a
read-only guard. No server to run, no wrapper needed: shell out to the CLI.

## Connect

    dbcrust <url> [flags] -c "<sql>"

| URL | Connects to |
|---|---|
| `postgres://user:pass@host:5432/db` | PostgreSQL (`mysql://`, `clickhouse://`, `mongodb://`, `elasticsearch://` likewise) |
| `sqlite:///path/to.db` or `./file.db` | SQLite |
| `./data.csv` `./data.parquet` `./data.json` | SQL over files via DataFusion (globs: `'parquet:///data/*.parquet'`) |
| `session://name` | saved session — credentials come from the keystore; best option for agents |
| `docker://container/db` | database running in a Docker container |
| `vault://role@mount/db` | HashiCorp Vault dynamic credentials |
| `recent://`, bare `file://` | interactive pickers — do NOT use non-interactively |

List saved sessions with `dbcrust -c '\s'`. A file literally named like a
subcommand (`agents`, `config`) must be opened as `sqlite://agents`.

## Execute

- `-c "<sql>"` — repeatable; one `-c` may hold several `;`-separated
  statements. Prefer one statement per `-c` for the simplest output parsing.
- `-c '\dt'` — backslash commands work non-interactively too.
- `-f file.sql` — repeatable, pure SQL only; runs interleaved with `-c` in
  command-line order.
- stdin: `echo "SELECT 1" | dbcrust <url>` runs the pipe as a SQL script.

## Output formats

`-o/--format table|expanded|csv|json|jsonl` (env: `DBCRUST_FORMAT`; default `table`).

`-o json` prints ONE single-line envelope per statement:

    {"columns":["id","name"],"rows":[["1","Alice"]],"row_count":1,"truncated":false}

- All values arrive as strings; NULL rendering is backend-dependent
  (`"NULL"` on PostgreSQL, `""` on SQLite).
- `truncated:true` means the output guards dropped rows (10k rows / 32MB).
- `-o jsonl` → one `{"col":"value"}` object per row; on truncation the final
  line is `{"_truncated":true,"_row_count":N}`.
- SQL and connection errors (under `-o json|jsonl`) are a single stderr line:
  `{"error":{"code":"query_error|connection_error|read_only_violation|…","message":"…"}}`
- Statements with no result set (DDL/DML) print nothing — rely on the exit code.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | a statement failed (the batch stops at the first failure) |
| 2 | usage / bad arguments |
| 3 | connection or URL-resolution failure |
| 4 | statement blocked by `--read-only` |
| 130 | interrupted (Ctrl-C) |

## Safety flags (recommended defaults for agents)

    dbcrust <url> --read-only --timeout 30 --max-rows 500 --no-input -o json -c "…"

- `--read-only` — rejects statements that could write (DML/DDL, writing
  PRAGMAs, `SELECT … INTO`, sequence bumps, `$out`/`$merge` Mongo stages, …)
  plus connect-level hardening on SQLite (`query_only`) and PostgreSQL
  (`default_transaction_read_only`). Best-effort textual guard: for hard
  guarantees connect with a read-only database role. `--read-only=false`
  overrides a `read_only_default = true` config.
- `--timeout SECS` — per-run query timeout (0 disables).
- `--max-rows N` — auto-LIMIT appended to top-level SELECTs (0 disables;
  default 100). A result with exactly N rows may have more behind the limit.
- `--no-input` — never open an interactive prompt; fail fast with a hint
  instead (implied when stdin is not a terminal).

## Discover schema

    dbcrust <url> -c '\ddl'               # compact CREATE TABLE dump, all tables (cap 100)
    dbcrust <url> -c '\ddl users orders'  # specific tables (schema-qualified ok)
    dbcrust <url> -o json -c '\dt'        # table list as JSON
    dbcrust <url> -o json -c '\d users'   # full column/index/FK detail as JSON
    dbcrust <url> -c '\l'                 # list databases

`\ddl` is usually the best first call: one invocation loads the whole schema
mental model in a token-efficient form.

## Tips

- Save a session once (`\ss prod` inside the REPL) and use `session://prod`
  everywhere — no credentials on the command line or in shell history.
- Named queries are parameterized shortcuts: `dbcrust <url> -c "top_users 10"`
  (`$1`, `$*` substitution; manage with `\n`, `\ns`, `\nd`).
- House policy: `dbcrust config set read_only_default true`.
- Privacy: dbcrust only talks to your database. Its own AI assistant
  (`??`, `\ai`) is a separate opt-in feature; nothing leaves the machine
  unless you configure that.

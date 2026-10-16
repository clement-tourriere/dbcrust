---
title: Safety & Guardrails
description: The read-only guard, limits, exit codes, and output contract for agent-driven DBCrust
---

Agents make mistakes; guardrails make them cheap. This page is the full contract an agent-driven `dbcrust` run obeys.


:::tip[Protecting credentials, not just statements]
Everything on this page runs the agent *as you*: it can reach any password your account can. For production databases enroll the connection with the [credential broker](/dbcrust/user-guide/agent-connections/) so the agent never holds one.
:::

## The read-only guard

```bash
dbcrust <url> --read-only -c "…"          # per run
dbcrust config set read_only_default true  # house policy; override with --read-only=false
```

`--read-only` rejects, per backend:

| Backend | Blocked |
|---|---|
| PostgreSQL / MySQL / SQLite / ClickHouse / files | anything that isn't a single read-only statement: DML/DDL, data-modifying CTEs, `EXPLAIN ANALYZE` on writes, multi-statement smuggling (`SELECT 1; DROP …`), `SELECT … INTO`, `INTO OUTFILE`, sequence bumps (`nextval`/`setval`), advisory/named locks, `PRAGMA name = value` assignments (read PRAGMAs stay allowed) |
| MongoDB | everything except a read allowlist (`find`, `findOne`, `aggregate` without `$out`/`$merge`, `count*`, `distinct`, `getIndexes`, `stats`, …) — unknown verbs are rejected by default |
| Elasticsearch | anything that isn't `SELECT`/`SHOW`/`DESCRIBE`/`EXPLAIN` (its SQL interface is read-only by design) |
| Redis / Valkey | everything except a read-command allowlist (`GET`, `HGETALL`, `LRANGE`, `SCAN`, `XRANGE`, `INFO`, …); `KEYS`, `FLUSHALL`, `CONFIG`, scripting, `SUBSCRIBE` and blocking commands are always refused, unknown and module commands are denied by default |
| SQL Server / Azure SQL | everything except a single `SELECT` / `WITH … SELECT`: writes and DDL, `SELECT … INTO`, `EXEC`/`sp_*`/`xp_*` procedures (`xp_cmdshell`), `OPENROWSET`/`OPENQUERY`/`OPENDATASOURCE`, `BULK`, linked-server four-part names, `DBCC`, `BACKUP`/`RESTORE`, `WAITFOR`, `USE`/`SET`, transactions, `GO` batches and several statements (T-SQL is lexed: brackets, `N'…'`, nested comments). A text check — SQL Server has no read-only session, so use a read-only login for a hard limit; see the [SQL Server guide](/user-guide/sqlserver/#read-only) |
| Redshift | everything except a single `SELECT` / `WITH … SELECT` / `SHOW` / `EXPLAIN`: writes and DDL, `COPY` / `UNLOAD` (S3), `CREATE EXTERNAL …`, `CALL`, `VACUUM`, `GRANT`/`REVOKE`, datashares, `SELECT … INTO`, `pg_terminate_backend`, several statements. A text check, plus each statement runs in `BEGIN READ ONLY` … `ROLLBACK`; Redshift has no read-only session, so use a `GRANT SELECT`-only user for a hard limit; see the [Redshift guide](/user-guide/redshift/#read-only) |
| libSQL / Turso | everything except a single `SELECT` / `WITH … SELECT` / `EXPLAIN` or an introspection `PRAGMA` (`table_info`, `index_list`, …): writes and DDL, `ATTACH`/`DETACH`, `VACUUM`, `load_extension()`, `PRAGMA` assignments and acting PRAGMAs, several statements. Each statement is also checked by the server (`describe` → `sqlite3_stmt_readonly`) before it runs. A read-only token is the hard limit; see the [libSQL guide](/user-guide/libsql/#read-only) |
| InfluxDB | InfluxQL and v3 SQL: only `SELECT` / `SHOW` / `EXPLAIN` (v3 also `WITH … SELECT`, `DESCRIBE`); `SELECT … INTO`, `DROP`, `DELETE`, `CREATE`, `ALTER`, `GRANT`, `KILL` and the like are refused wherever they appear. Flux: imports are allow-listed (`http`, `sql`, `socket`, `kafka`, `csv`, `secrets`, `tasks`, `monitor`… are refused), `to()` / `experimental.to()` / `influxdb.wideTo()` and `from(host:, token:)` are refused. Lexed (strings, regexes, comments and quoted names never count). A text check: InfluxDB 1.x does not enforce read-only for `GET` (a write sent that way still runs, with only a deprecation warning), so use a v1 user with `READ` privileges or a read-only v2/v3 API token for a hard limit; see the [InfluxDB guide](/user-guide/influxdb/#read-only) |
| Amazon S3 / R2 / MinIO | the seven S3 commands (`ls`, `head`, `get`, `buckets`, `cd`, `pwd`, `help`) and nothing else: none of them writes, so there is no write to block, and anything that is not one of them (`rm`, `put`, SQL…) is refused. Read-only *by construction* of the client, not by the server: what the key may do is IAM, so give it a read-only policy for a hard limit; see the [S3 guide](/user-guide/s3/#read-only-by-construction) |
| Amazon DynamoDB | one PartiQL `SELECT`, and the commands `tables`, `describe`, `scan`, `get`, `query`, `help`: a lexer reads the statement through comments, quoted identifiers and string literals, so `INSERT` / `UPDATE` / `DELETE` / `EXISTS(…)`, a second statement after a `;` and a write hidden behind a comment are refused, and the driver runs exactly what the guard accepts. Writes are not offered at all. Read-only *by construction* of the client, not by the server: what the key may do is IAM (add `dynamodb:FullTableScan = false` on `PartiQLSelect` to forbid scans server-side); see the [DynamoDB guide](/user-guide/dynamodb/#read-only-by-construction) |
| Amazon CloudWatch Logs | Logs Insights queries (whose first command is one of `fields`, `filter`, `stats`, `parse`, `sort`, `limit`, `display`, `dedup`, `pattern`, `diff`, `unnest`, `anomaly`) over a time window, and `groups`, `streams`, `tail`, `use`, `help`: no write API (`PutLogEvents`, group or stream management, retention, export, subscription filters) is reachable, and anything else (`put`, `delete`, SQL…) is refused. A query is billed per GB scanned, so it always has a window, at most 50 groups, a result limit and an optional scan budget, and it is stopped on Ctrl-C and on timeout. Read-only *by construction* of the client, not by the server; application logs may hold personal data, so use CloudWatch data protection policies; see the [CloudWatch Logs guide](/user-guide/cloudwatch-logs/#read-only-by-construction) |
| DuckDB | everything except `SELECT` / `WITH … SELECT` / `FROM … SELECT` / `VALUES` / `SHOW` / `DESCRIBE` / `SUMMARIZE` / `PIVOT` / `EXPLAIN` and read-only `PRAGMA`s (`table_info`, `database_size`, `show_tables`…): writes and DDL, `COPY`, `EXPORT`/`IMPORT DATABASE`, `ATTACH`/`DETACH`, `INSTALL`/`LOAD`, `SET`/`RESET`, `CALL`, `CHECKPOINT`, and the file/network/environment functions (`read_csv`, `read_parquet`, `glob`, `sqlite_scan`, `getenv`, `duckdb_secrets`, `query`…) and files named as tables (`FROM 'x.csv'`). **Engine-enforced** for a database file: `--read-only` opens it with `access_mode=READ_ONLY`, external access off and the configuration locked, so DuckDB itself refuses writes, file access and extensions; an in-memory database cannot be opened read-only. See the [DuckDB guide](/user-guide/duckdb/#read-only) |
| White Dragon | all non-empty SQL, qualifier-language, and structured JSON searches are allowed; DBCrust only calls White Dragon's read-only `/v1/search` endpoint |

A blocked statement exits with **code 4** and (under `-o json`) a `read_only_violation` error on stderr.

### Connect-level hardening

Where the driver supports it, read-only is also enforced below the statement guard, on **every pooled connection**:

- **SQLite** — `PRAGMA query_only=ON` at connect time (the server-side backstop that catches anything the textual guard misses)
- **PostgreSQL** — `default_transaction_read_only=on` session option

### Honest limits

The statement guard is **textual and best-effort**. A `SELECT` can still call a user-defined function with side effects, and MySQL/ClickHouse/MongoDB have no connect-level backstop here. For hard guarantees, do what DBAs have always done:

- connect with a **read-only database role**, or
- point the agent at a **replica**.

`--read-only` then remains useful as fast, local, zero-config defense in depth.

## Limits and timeouts

```bash
dbcrust <url> --timeout 30 --max-rows 500 -c "…"
```

- `--timeout SECS` — per-run query timeout (0 disables; config: `query_timeout_seconds`).
- `--max-rows N` — auto-`LIMIT` appended to top-level `SELECT`s (0 disables; config: `default_limit`, default 100). A result with exactly N rows may have more behind the limit — agents should treat `row_count == max_rows` as "possibly truncated by LIMIT".
- Output guards cap any rendering at 10,000 rows / 32 MB; `-o json` reports it via `"truncated":true`.

## Never hang

`--no-input` (implied whenever stdin is not a terminal) turns every would-be interactive prompt — password entry, `session://`/`recent://`/`file://`/Vault pickers — into an immediate, hint-bearing error instead of a hung process. The pager is disabled in one-shot mode for the same reason.

## The output contract

- **stdout**: results only. One rendering per statement; statements without result sets print nothing.
- **stderr**: status ("Connecting to saved session…"), warnings, and errors. Under `-o json|jsonl`, SQL/connection errors are a single JSON line: `{"error":{"code":"…","message":"…"}}` with codes `query_error`, `connection_error`, `read_only_violation`, `file_error`, `usage_error`.
- **Exit codes**:

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | a statement failed (batch stops at first failure) |
| 2 | usage / bad arguments |
| 3 | connection or URL-resolution failure |
| 4 | statement blocked by `--read-only` |
| 130 | interrupted |

The Python CLI (`pip install dbcrust`) maps its exceptions to the same codes.

## Privacy

Agent-driven DBCrust talks **only to your database**. It performs no telemetry and no AI calls. DBCrust's own AI assistant (`??`, `\ai`) is a separate, disabled-by-default feature — see [its privacy notes](/dbcrust/user-guide/ai-assistant/) if you enable it.

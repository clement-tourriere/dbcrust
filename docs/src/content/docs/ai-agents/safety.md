---
title: Safety & Guardrails
description: The read-only guard, limits, exit codes, and output contract for agent-driven DBCrust
---

Agents make mistakes; guardrails make them cheap. This page is the full contract an agent-driven `dbcrust` run obeys.

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

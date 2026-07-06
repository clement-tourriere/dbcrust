---
title: DBCrust for AI Agents
description: One CLI for every data service — the agent-friendly alternative to running an MCP server per database
---

AI coding agents (Claude Code, Codex, Cursor, and custom harnesses) are at their best when they can shell out to a well-behaved CLI. DBCrust gives them **one binary for every data service** — PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB, Elasticsearch, and Parquet/CSV/JSON files — with a one-shot mode designed for programmatic callers.

```bash
dbcrust session://prod -c '\ddl'                            # whole schema as compact DDL
dbcrust session://prod --read-only -o json -c "SELECT …"    # structured results
echo "SELECT count(*) FROM logs" | dbcrust ./logs.parquet   # even files are databases
```

## Why a CLI instead of an MCP server?

MCP is a fine protocol, but for database access an agent-grade CLI is usually the lighter tool:

- **Nothing to run or configure.** No server process per project, no JSON config wiring, no restart when it wedges. If `dbcrust` is on `PATH`, every agent on the machine can use it.
- **No standing token tax.** MCP tool schemas ride along in every conversation. A CLI costs tokens only when used — and `dbcrust agents` prints a compact contract the agent reads once.
- **Composable.** Output pipes into `jq`, `grep`, files, and other CLIs. Exit codes drive shell logic (`&&`, `||`, retries).
- **Harness-agnostic.** The same invocation works in Claude Code, Codex, Cursor, CI jobs, and plain bash scripts.

MCP still makes sense when the agent has no shell (hosted assistants), or when you need per-tool OAuth scoping. For everything else: it's just a CLI call away.

## What makes DBCrust agent-grade

| Need | DBCrust answer |
|---|---|
| Structured output | `-o json` (compact envelope) · `-o jsonl` · `-o csv` · env `DBCRUST_FORMAT` |
| Clean stdout | one-shot mode prints results only; status goes to stderr; the pager never engages |
| Machine-readable errors | single-line `{"error":{"code","message"}}` on stderr under `-o json` |
| Stable exit codes | 0 ok · 1 SQL error · 2 usage · 3 connection · 4 blocked by `--read-only` · 130 interrupt |
| Schema discovery | `\ddl` compact DDL dump · `\dt` / `\d table` are JSON-aware |
| Guardrails | `--read-only` (+ connect-level hardening) · `--timeout` · `--max-rows` |
| Never hangs | `--no-input` fails fast with a hint instead of prompting (implied when stdin isn't a TTY) |
| Credential hygiene | `session://name` — no secrets in argv; Vault, pgpass, encrypted `.dbcrust` also supported |
| Scripts | `-f file.sql` (interleaved with `-c` in argv order) and stdin pipes |
| Self-documentation | `dbcrust agents` prints the full contract in ~100 lines |

## Two directions, one tool

Don't confuse the two AI stories here:

- **This section** is about *external agents driving DBCrust* — dbcrust as the agent's database tool. No AI configuration is needed, and nothing leaves your machine.
- **[The AI assistant](/dbcrust/user-guide/ai-assistant/)** is the inverse: *DBCrust's own opt-in AI* (`??` text-to-SQL, `???` investigations), which calls an LLM provider you configure.

They compose nicely — an agent can even invoke `dbcrust "$URL" -c '\ddl'` to ground its own reasoning — but only the second one ever talks to an AI provider.

## Next steps

- [Quickstart](/dbcrust/ai-agents/quickstart/) — install, teach your agent, paste-ready snippets
- [Safety & guardrails](/dbcrust/ai-agents/safety/) — the read-only guard, limits, and the full output contract

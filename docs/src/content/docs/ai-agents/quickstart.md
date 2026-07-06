---
title: Agent Quickstart
description: Install DBCrust and teach your coding agent to use it in two minutes
---

## 1. Install

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://clement-tourriere.github.io/dbcrust/install.sh | sh
# or: uv tool install dbcrust · pipx install dbcrust · cargo install dbcrust
```

Verify: `dbcrust --version && dbcrust agents | head`.

## 2. Teach your agent

The binary documents its own contract — an agent can bootstrap itself with one call:

```bash
dbcrust agents
```

For a durable setup, paste this block into your project's agent memory file — `CLAUDE.md` (Claude Code), `AGENTS.md` (Codex and others), or your Cursor rules:

```markdown
## Database access (dbcrust)

Use `dbcrust` for all database work — one CLI for PostgreSQL, MySQL, SQLite,
ClickHouse, MongoDB, Elasticsearch, and Parquet/CSV/JSON files.

- Connect: `dbcrust 'session://dev'` (saved session) or any URL
  (`postgres://…`, `mysql://…`, `./data.csv`, `./app.sqlite`)
- Default flags: `--read-only --no-input -o json`
  (drop `--read-only` only when a write is explicitly intended)
- Discover schema first: `dbcrust <url> -c '\ddl'` → compact DDL for all tables
- Query: `dbcrust <url> -o json -c "SELECT …"` → one single-line
  `{"columns":[…],"rows":[[…]],"row_count":n,"truncated":bool}` envelope per statement
- Exit codes: 0 ok · 1 SQL error · 2 usage · 3 connection · 4 blocked by --read-only
- Errors under `-o json` arrive as `{"error":{"code","message"}}` on stderr
- Full contract: run `dbcrust agents`
```

**Claude Code tip:** allow the tool once with a permission rule like `Bash(dbcrust *)` in `.claude/settings.json`, or approve interactively on first use.

## 3. Set up credentials once

Agents should never paste passwords into argv. Pick one:

```bash
dbcrust postgres://user@host/db        # prompts once, saves encrypted to ~/.dbcrust
# then inside the REPL:  \ss dev      → saved session
```

From then on the agent connects with `dbcrust session://dev` — no secrets in shell history, no prompts to hang on (`--no-input` fails fast with a hint if anything would prompt).

Vault users: `vault://role@mount/database` gets dynamic credentials with zero agent-visible secrets.

## 4. The loop in practice

A typical agent exchange looks like:

```bash
# 1. Ground yourself in the schema (one call, token-compact)
dbcrust session://dev -c '\ddl'

# 2. Query with structured output and guardrails
dbcrust session://dev --read-only --no-input -o json \
  -c "SELECT status, count(*) AS n FROM orders GROUP BY status"
# → {"columns":["status","n"],"rows":[["paid","9421"],["refunded","103"]],"row_count":2,"truncated":false}

# 3. Branch on exit codes
dbcrust session://dev --read-only -o json -c "$SQL" || case $? in
  1) echo "SQL error — read stderr JSON";;
  3) echo "connection problem";;
  4) echo "write blocked — rerun with --read-only=false if intended";;
esac
```

Works identically against files (`dbcrust ./export.parquet -o json -c "SELECT …"`) and every other backend.

## 5. Going further

- `-o jsonl` streams one object per row — handy for `jq`-style row processing.
- `-f migration_check.sql` runs script files; stdin pipes work too.
- Named queries give agents parameterized, pre-approved shortcuts: `dbcrust <url> -c "top_users 10"`.
- The [Python API](/dbcrust/python-api/overview/) exposes the same engine (`dbcrust.run_command([...])`) when your agent framework prefers in-process calls.
- Full guardrail details: [Safety & guardrails](/dbcrust/ai-agents/safety/).

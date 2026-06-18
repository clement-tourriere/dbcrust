---
title: AI Assistant
description: Natural-language SQL generation with multi-provider AI support
---

DBCrust includes an opt-in AI assistant that turns natural language into SQL, directly in the interactive session. Type `??` followed by what you want, and the assistant generates the query using your database's actual schema as context:

```sql
dbcrust postgres://localhost/shop

?? top 10 customers by total order value this year
```

```sql
SELECT c.name, SUM(o.total) AS total_value
FROM customers c
JOIN orders o ON o.customer_id = c.id
WHERE o.created_at >= date_trunc('year', now())
GROUP BY c.name
ORDER BY total_value DESC
LIMIT 10;
```

The generated SQL is shown first and only runs after you confirm (see [execution modes](#execution-modes)).

## Setup

AI features are **disabled by default**. Run the interactive wizard once:

```sql
\ai setup
```

The wizard walks you through:

1. **Provider** — Anthropic, OpenAI, Gemini, Ollama, Groq, DeepSeek, xAI, OpenRouter, Z.AI, GitHub Copilot, Cohere, or Together. The choice is saved (`provider` under `[ai]`) and drives authentication and routing.
2. **Authentication** — an API key stored in your OS keychain, an encrypted file, or an environment variable. For OpenAI you can instead [sign in with ChatGPT](#sign-in-with-chatgpt) and use your subscription.
3. **Model** — picked from a **live list fetched from your provider** (so it reflects what your key can access), with type-to-filter and a free-text escape hatch. Falls back to curated suggestions when the list can't be fetched.

Local providers like Ollama need no API key — just a model name and optionally an `endpoint`.

Pressing `Ctrl-C` anywhere in the wizard cancels the whole setup without saving; `Esc` skips the current step where a skip makes sense.

## Using `??`

| Input | What happens |
|-------|--------------|
| `?? show all users created last week` | Generates SQL from your schema, asks to execute |
| `?? now only the active ones` | Follow-ups work — the last 5 exchanges are kept as conversation context |
| `\ai clear` | Reset the conversation history |

Schema context is built from your current database: table and column metadata for up to `max_schema_tables` tables (50 by default). **No row data is ever sent to the provider** — only schema metadata and your question.

Responses stream to the terminal as they arrive (`streaming = true`); press `Ctrl-C` to cancel a generation in progress.

## Investigating with `???`

Where `??` does one-shot text-to-SQL, `???` runs an **agentic investigation loop**: the assistant calls read-only tools, observes the results, and iterates until it can answer a question with evidence — ideal for "why is this slow?" questions.

```sql
??? counting rows on the orders table with a join is slow — why?
```

The agent works through tools and narrates its progress (dim lines), then prints a structured answer:

```
🔍 Investigating with claude-sonnet-4-6… (Ctrl-C cancels)
🔧 describe_table: orders
   🗂  orders: 9 cols
🔧 explain: SELECT count(*) FROM orders o JOIN customers c …
   📊 8 rows × 1 cols

## Finding
The join sequentially scans `orders` because `orders.customer_id` has no index.
## Evidence
The plan shows a Seq Scan on orders (rows=1.2M) feeding a Hash Join.
## Recommendation
CREATE INDEX idx_orders_customer_id ON orders (customer_id);
```

The agent has four tools: `list_tables`, `describe_table`, `run_sql`, and `explain`. Key properties:

- **Read-only.** Every query is gated — only `SELECT`/`WITH`/`SHOW`/`EXPLAIN` run; writes, DDL, and **many known side-effecting patterns** (sequence bumps, named/advisory locks, notifications, `SELECT … INTO`, file writes, mutating `PRAGMA`) are rejected back to the model, which self-corrects. So it runs without per-step confirmation. This is **best-effort SQL inspection, not a hard sandbox** — a `SELECT` can still call a user-defined side-effecting function. For sensitive databases, point the agent at a **read-only role or a replica** for real enforcement.
- **Bounded.** It takes at most `agentic_max_iterations` tool turns (8 by default), and each tool result is capped to `agentic_max_rows_per_tool` rows (50). If it hits the limit it is forced to summarize rather than stop silently.
- **Cancelable.** `Ctrl-C` stops the loop at any point.
- **Remembers its own context.** `???` keeps a conversation history **separate from `??`**, so follow-up investigations build on earlier ones without polluting text-to-SQL prompts. `\ai clear` resets both.
- **Works with API keys or a ChatGPT subscription.** The loop runs over streaming requests, so it works on the Codex/subscription backend (`\ai login`) as well as any API-key provider.

## Execution modes

The assistant never silently runs writes unless you explicitly opt in:

| Mode | Behavior |
|------|----------|
| `confirm` (default) | Always ask before executing. Defaults to **Yes** for `SELECT`, **No** for writes |
| `auto_select` | Run `SELECT` statements automatically; ask (default **No**) for anything else |
| `auto_execute` | Run everything without asking — use with care |

Set it in the config file (`execution_mode` under `[ai]`) or during `\ai setup`.

## `\ai` commands

| Command | Description |
|---------|-------------|
| `\ai` or `\ai status` | Show provider, model, credential status, and settings |
| `\ai setup` | Interactive setup wizard |
| `\ai provider [name\|auto]` | Set the active provider (`auto` = infer from the model name) |
| `\ai model [name]` | Switch model — without an argument, pick from the provider's live model list |
| `\ai login` | Sign in with ChatGPT (use your subscription instead of an API key) |
| `\ai logout` | Sign out of ChatGPT and return to API-key auth |
| `\ai on` / `\ai off` / `\ai toggle` | Enable / disable AI features |
| `\ai clear` | Clear the conversation history |

## Sign in with ChatGPT

If you have a ChatGPT plan (Plus, Pro, Business, …), the assistant can use it directly instead of a pay-per-use OpenAI API key:

```sql
\ai login
```

This opens your browser for an OAuth sign-in (the same flow Codex CLI uses), stores the tokens in your OS keychain (encrypted-file fallback), and routes requests through the ChatGPT Codex backend on your plan's quota. If you already ran `codex login`, the setup wizard also offers to **reuse your Codex CLI session** — no second sign-in needed.

Things to know:

- Model choice is limited to what that backend serves (`gpt-5.5`, `gpt-5-codex`, …); the picker shows the supported set.
- `\ai logout` deletes the stored tokens and returns to API-key auth.
- This rides an OpenAI surface that is tolerated but not officially documented for third-party tools — it can change or break without notice. dbcrust only ever *reads* `~/.codex/auth.json`, never writes it.

## Providers and models

Provider handling is delegated to the [genai](https://crates.io/crates/genai) crate (25+ providers over their native protocols). The active provider is whatever `provider` is set to under `[ai]`; with `provider = "auto"` it is inferred from the model name (`claude-*` → Anthropic, `gpt-*` → OpenAI, …). `provider::model` syntax still forces it per-model:

```sql
\ai model groq::llama-3.1-70b
```

`\ai model` without an argument fetches the **live model list** from your provider's `/models` endpoint using your stored key — so restricted keys only show what they can use — and falls back to curated suggestions when the endpoint is unreachable.

For self-hosted gateways, Ollama, LM Studio, or any OpenAI-compatible service, set a custom endpoint in the config:

```toml
[ai]
model = "llama3.2"
endpoint = "http://localhost:11434/v1/"
```

## API key storage

Keys are resolved in order:

1. **Environment variable** — the provider's standard name (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …)
2. **OS keychain** — stored under the `dbcrust` service
3. **Encrypted file** — AES-GCM encrypted, in the DBCrust config directory

`\ai setup` lets you pick where to store the key. Keys never appear in `config.toml`.

## Configuration reference

All settings live under `[ai]` in `~/.config/dbcrust/config.toml`:

```toml
[ai]
enabled = false                # opt-in; \ai setup or \ai on enables it
provider = "auto"              # "auto" infers from the model name, or e.g. "openai"
model = "claude-sonnet-4-6"    # model identifier
auth_method = "api_key"        # api_key | chatgpt_subscription (OpenAI, via \ai login)
# endpoint = "http://..."      # custom/self-hosted endpoint (optional)
max_tokens = 4096
temperature = 0.0
streaming = true               # stream responses as they arrive
max_schema_tables = 50         # cap on tables sent as schema context
show_generated_sql = true      # display SQL before/after generation
execution_mode = "confirm"     # confirm | auto_select | auto_execute
history_length = 5             # conversation exchanges kept for follow-ups
agentic_max_iterations = 8     # max tool-call turns for ??? investigations
agentic_max_rows_per_tool = 50 # rows from one ??? tool query fed back to the model
```

## Django-aware AI

In a Django project, the AI can investigate with your **models and ORM code** as context — not just the raw SQL schema — so it recommends Django-level fixes (`select_related` / `prefetch_related` / `only` / `db_index` / `Meta.indexes`) with exact `file:line` references.

A management command for ad-hoc questions:

```bash
python manage.py dbcrust_ai "why is the order list view slow?"
```

Or, inside an analysis block, so the agent also sees the **actual captured queries** and the code that issued them:

```python
from dbcrust.django import analyze

with analyze() as a:
    list(Order.objects.all())          # exercise the slow path
print(a.investigate_ai("why are there so many queries here?"))
```

Both reuse your `\ai setup` configuration and run the same read-only agent against your Django database (API-key or ChatGPT-subscription auth — same as `???`). If Django runs with a different `HOME` than your shell, point it at the CLI config directory with `DBCRUST_CONFIG_DIR=/path/to/.config/dbcrust` (or `DBCRUST_CONFIG_DIR = "/path/to/.config/dbcrust"` in Django settings). Auth secrets must also be available in that runtime; containers/services usually need API-key environment variables or their own `\ai setup` / `\ai login`.

## Privacy notes

What is sent to the configured provider depends on which feature you use:

- **`??` (text-to-SQL):** your question, recent conversation history, and schema **metadata** (table/column names and types). Query **results are never sent** — the generated SQL runs locally, after generation.
- **`???` (agentic) and the Django dashboard's "Investigate with AI":** the same metadata **plus the output of the read-only queries the agent runs** — i.e. up to `agentic_max_rows_per_tool` (default 50) **rows of actual data per query**, query plans, and (for the Django-aware paths) your **model definitions, source `file:line` locations, and the captured SQL** of flagged queries. This is what lets the agent reason with evidence, but it means real row data can leave the machine automatically. Lower `agentic_max_rows_per_tool`, or avoid `???` on sensitive tables, if that matters.
- For air-gapped or sensitive environments, use a local provider (Ollama / LM Studio) via `endpoint`.

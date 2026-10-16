# dbcrust-agent

A thin, **credential-free** DBCrust client for AI coding agents (Claude Code,
Codex, Cursor, …).

The database owner runs the DBCrust **broker** — as a system service under its
own OS user (`sudo dbcrust broker install-service`, recommended) or as a
same-user process (`dbcrust broker serve`, local development) — which holds the
credentials and exposes only owner-enrolled *aliases*, enforces read-only mode,
row and time limits, and audits every request. `dbcrust-agent` talks to that
broker over a Unix socket and nothing else. The full guide — modes, guarantees,
database role recipes, MCP setup, troubleshooting — is
[Agent connections](https://clement-tourriere.github.io/dbcrust/user-guide/agent-connections/):

* it links **no keystore, no database drivers, no config storage** and does not
  depend on the `dbcrust` crate — there are no secrets in its address space,
  and the wire protocol has no field that could carry one;
* it only names aliases; it cannot supply a URL, host or password.

## Commands

```bash
dbcrust-agent list                                  # aliases, engine, access, limits
dbcrust-agent tables prod
dbcrust-agent describe prod public.users
dbcrust-agent query prod -c 'SELECT * FROM users' -o json --max-rows 50 --timeout 10
dbcrust-agent explain prod -c 'SELECT * FROM users WHERE id = 1'
dbcrust-agent doctor                                # socket, handshake, broker version, alias count
dbcrust-agent mcp                                   # MCP server on stdio
```

Global flags: `--socket PATH`, `--client LABEL` (audit label; for `mcp` it
defaults to the MCP client's own name).

### Output (`-o table|csv|json|jsonl`)

Same contract as DBCrust's one-shot mode (`dbcrust agents`): `json` prints one
single-line `{"columns":[…],"rows":[[…]],"row_count":N,"truncated":false}`
envelope (NULL is `null`); `jsonl` one object per row plus a final
`{"_truncated":true,"_row_count":N}` line when truncated; `csv` has a header.
`list`, `tables` and `describe` use the same envelope. Errors go to stderr as
`{"error":{"code":"…","message":"…"}}` for `json`/`jsonl`, plain text otherwise.

### Exit codes

| code | meaning |
|---|---|
| 0 | success |
| 1 | `query_error` (also `unsupported_engine`, `internal`) |
| 2 | usage, protocol/version mismatch, `unsupported_platform` |
| 3 | connection / socket not found / `broker_locked` |
| 4 | `read_only_violation`, `ddl_refused`, `multi_statement_refused` |
| 5 | `denied`, `unknown_alias`, `limit_exceeded` |
| 124 | `timeout` |

### Finding the broker

`--socket PATH`, else `$DBCRUST_BROKER_SOCKET` (a path), is used alone: if
it doesn't exist the command fails rather than reach another broker. Without
either, the first existing socket wins:

1. the service socket: `/run/dbcrust/broker.sock` on Linux, `/var/run/dbcrust/broker.sock` on macOS
2. `$XDG_RUNTIME_DIR/dbcrust/broker.sock` (when set)
3. `<config dir>/broker/broker.sock`, config dir = `$DBCRUST_CONFIG_DIR` or `~/.config/dbcrust`

Only `DBCRUST_BROKER_SOCKET`, `XDG_RUNTIME_DIR`, `DBCRUST_CONFIG_DIR` and
`HOME` are ever read from the environment. Unix only: on Windows every command
exits 2 with `unsupported_platform`.

## MCP registration

Tools: `list_connections`, `list_tables`, `describe_table`, `query`, `explain`.

**Claude Code**

```bash
claude mcp add dbcrust -- dbcrust-agent mcp
```

**Codex** — `~/.codex/config.toml`

```toml
[mcp_servers.dbcrust]
command = "dbcrust-agent"
args = ["mcp"]
```

**Cursor** — `mcp.json`

```json
{"mcpServers": {"dbcrust": {"command": "dbcrust-agent", "args": ["mcp"]}}}
```

## Keeping it thin

`Cargo.toml` forbids heavy dependencies. Check with:

```bash
mise exec -- cargo tree --locked -p dbcrust-agent -e normal
```

The protocol is documented in
[`../agent-protocol/PROTOCOL.md`](../agent-protocol/PROTOCOL.md).

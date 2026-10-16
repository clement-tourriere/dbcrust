---
title: "Agent Connections (Credential Broker)"
description: "Let AI coding agents query your databases without ever holding a password: the DBCrust broker, enrolled aliases, read-only policy, audit log, and MCP setup for Claude Code, Codex and Cursor."
---

# Agent Connections

AI coding agents are useful with a database in reach — and dangerous with its password in reach. DBCrust's **credential broker** splits the two. You enroll a connection once under an *alias*; the broker keeps the password, connects on the agent's behalf, forces read-only access (unless you opt in to writes), caps rows and time, and writes an audit line for every request. The agent only ever talks to a small, credential-free client, `dbcrust-agent`, and only ever sees aliases.

```text
 agent (Claude Code, Codex, Cursor, …)
    │  MCP over stdio, or one-shot CLI
    ▼
 dbcrust-agent ──► Unix socket ──► broker ──► your database
 (no keystore, no          (peer uid/gid     (holds the password; classifies,
  drivers, no config)        checked)         limits and audits every statement)
```

This is the recommended way to give an agent access to anything that matters. Directly running `dbcrust postgres://…` from an agent still works (see [Safety & guardrails](/dbcrust/ai-agents/safety/)), but then the agent runs *as you* and can reach whatever your account can.

## Concepts

| Term | Meaning |
|------|---------|
| **Broker** | The process that holds credentials and executes requests: `dbcrust broker serve`, or the system service installed by `install-service`. |
| **Alias** | The name an agent uses (`prod`, `analytics`). It maps to one exact target (engine, host, port, database, user, TLS mode) that *you* enrolled. The agent cannot supply a URL, host or password. |
| **Enrollment** | `dbcrust broker enroll`: you type the database password once; the broker stores it together with the target it is bound to and the policy ceiling. |
| **Policy** | Per alias: read-only or read-write, maximum rows (default 500, cap 10 000), statement timeout (default 30 s, cap 600 s), maximum result bytes (default 8 MiB, cap 64 MiB). |
| **Audit** | `audit.jsonl`: one line per decision. Hashes and lengths of statements, never SQL text, parameters or results. |
| **Thin client** | `dbcrust-agent`: an MCP server (`dbcrust-agent mcp`) and a one-shot CLI. It links no keystore, no database drivers and no config storage, and the wire protocol has no field that could carry a credential. |

What the broker accepts for enrollment today: PostgreSQL, MySQL/MariaDB, SQLite and ClickHouse. Other engines are refused with `unsupported_engine` (Redis says *"Redis enrollment for agents is not supported yet"*: it needs a command classifier of the broker's own first; SQL Server says *"SQL Server enrollment for agents is not supported yet: use a read-only login and wait for the read-only probe"*: it has no session-level read-only mode, so enrollment must first prove the login cannot write; Redshift says *"Redshift enrollment for agents is not supported yet: the server has no session-level read-only mode to enforce"*: `BEGIN READ ONLY` is per transaction and cannot be proven from the outside, so only the user's grants limit an agent; libSQL says *"libSQL enrollment for agents is not supported yet: a read-only token is the only server-enforced limit and cannot be proven from the client"*; InfluxDB says *"InfluxDB enrollment for agents is not supported yet: use a read-only API token or a v1 user limited to reads"*: the broker's classifier and row-limit wrapper are SQL-only, and a v2/v3 token's scope cannot be proven read-only from the client).

## Choosing a mode

| | **Service mode** (recommended) | **User mode** (local development) |
|---|---|---|
| Broker runs as | its own OS user (`_dbcrust` on macOS, `dbcrust` on Linux) under launchd / systemd | you |
| Passwords stored in | a 0700 directory owned by the service user, in an encrypted file | your secret store (macOS keychain; encrypted file on Linux) |
| Set up with | `sudo dbcrust broker install-service` | nothing: `dbcrust broker enroll` then `dbcrust broker serve` |
| Starts | at boot | when you run it |
| Use it for | real credentials, shared machines, anything an agent should not be able to read | trying it out, throwaway or low-value databases |

### What each mode guarantees

| Question | Service mode (macOS, Linux) | User mode, macOS | User mode, Linux |
|---|---|---|---|
| Can the agent read the stored password? | **No.** The store and its key belong to the service user; your account cannot read them. | **Not silently.** The keychain asks before any other binary reads the item. Residual risks: you click Allow; a debugger attach when DevToolsSecurity is enabled. | **Possibly yes.** There is no per-application access control; anything running as you can read what you can read. Convenience only. |
| Can it point the alias at another host to capture the password? | **No.** The password is bound to the exact target; changing the target needs re-enrollment with the password. | **No** (same binding). | **No**, unless it can also read the secret. |
| Can it loosen the policy (more rows, allow writes)? | **No.** Loosening needs `sudo` and the password. | **No.** Re-enrollment needs the password. | **No** without the password. |
| Can it bypass the policy? | **No** for broker aliases. Your own *human* saved sessions remain reachable through the regular `dbcrust` CLI. Don't keep production admin credentials in the account the agent runs as. | Weak. The agent can run the trusted binary or start its own broker; broker records are refused by the regular CLI and the ceiling is read from the stored record, so this is not a way past the ceiling. | Convenience-grade. |

### What is *not* guaranteed, in any mode

- **root / administrators.** Anyone with root on the machine can read the service user's files or attach to the process. The broker is a boundary against an agent running as you, not against the operator of the machine.
- **A compromised broker binary or host.** Service mode refuses to run from a binary a normal user can modify (see [the trusted binary](#the-trusted-binary)); it cannot defend against a tampered package you installed yourself.
- **Data exfiltration through allowed `SELECT`s.** An agent can read everything the database role can read, and what it reads goes to your model provider as part of the conversation. Limits slow this down; only a least-privilege role (below) bounds it.
- **A perfect SQL classifier.** The broker classifies every statement with a lexer-based, default-deny classifier (multi-statement, DDL/DCL/admin, `SELECT … INTO`, `COPY … PROGRAM`, file/network functions are refused; anything it cannot classify is refused). It is a strong guard, but it is a *second* line of defence behind the connection's native read-only settings and, above all, the database role's own grants.
- **Cancellation on every engine.** Statement timeouts are enforced by the broker and, where the engine supports it, by the server; a driver that cannot cancel server-side may leave a statement running until it finishes.
- **Perfect memory hygiene.** Secrets are zeroized where possible, not everywhere.
- **A tamper-proof audit log.** In service mode the log is owned by the service user, so the agent cannot edit it, but root can. In user mode the agent can delete it.

## Quick start: service mode

Run this from a `dbcrust` you trust (a fresh download or your own build): the installer copies *that* binary to a root-owned location.

```bash
# See every action first (no root needed)
dbcrust broker install-service --dry-run

# Install and start the daemon
sudo dbcrust broker install-service
```

The installer, on **macOS**: creates the hidden `_dbcrust` user and the `dbcrust-agents` group, offers to add you to that group, copies the binary to `/usr/local/libexec/dbcrust-broker`, creates `/Library/Application Support/dbcrust-broker` (0700, owned by `_dbcrust`), writes `/Library/LaunchDaemons/dev.dbcrust.broker.plist` and loads it. On **Linux** it does the same with a `dbcrust` system user, a hardened `dbcrust-broker.service` unit and `systemctl enable`. It is safe to re-run to upgrade: it replaces the binary and the service definition and never touches your stored credentials.

Group membership applies to *new login sessions*: log out and back in before agents can connect.

```bash
# Enroll a connection. Admin commands run through the installed binary:
sudo /usr/local/libexec/dbcrust-broker broker enroll prod \
  postgres://agent_ro@db.internal:5432/app --mode service \
  --description "read replica"
# → prompts for the password (hidden); it is never taken from arguments or the environment

sudo /usr/local/libexec/dbcrust-broker broker list --mode service
dbcrust broker status --mode service        # daemon, trusted binary, socket, your group membership

# Try it as the agent would
dbcrust-agent list
dbcrust-agent query prod -c 'SELECT count(*) FROM users' -o json
```

Enrolled aliases are picked up by the running daemon automatically.

### The trusted binary

Anything that runs with the service's authority must not be replaceable by a normal user. In service mode, `broker serve` and every admin command run as root (or as the service user) **refuse to start** unless the executable *and every parent directory* are owned by root and not writable by group or others. That is why the daemon and `sudo` admin commands use `/usr/local/libexec/dbcrust-broker` and not `~/.cargo/bin/dbcrust`, which any process of yours — an AI agent included — could overwrite.

If you see `refusing to run the service-mode broker from an untrusted binary`, use the installed copy as shown above. `--allow-untrusted-binary` bypasses the check for development and prints a loud warning; don't use it with real credentials.

### Uninstalling

```bash
sudo dbcrust broker uninstall-service            # stops the daemon; keeps credentials and accounts
sudo dbcrust broker uninstall-service --purge    # also deletes the state dir (the stored credentials!) and the user/group; asks to confirm
```

## Quick start: user mode

```bash
dbcrust broker enroll scratch sqlite:///tmp/scratch.db
dbcrust broker enroll prod postgres://agent_ro@localhost:5432/app    # asks for the password
dbcrust broker serve            # leave it running in a terminal; Ctrl-C stops it
```

In another terminal (or from the agent): `dbcrust-agent list`, `dbcrust-agent query prod -c '…' -o json`.

State lives in `~/.config/dbcrust/broker/`, secrets in your secret store under `broker:<alias>` ids (see [Secret storage](/dbcrust/user-guide/secret-storage/)). On Linux with the encrypted-file backend the broker asks for the passphrase once at start, if it has a terminal; otherwise agents get `broker_locked` until you run `dbcrust secrets unlock` and restart it.

## Least-privilege database roles

The broker's read-only mode is a guard; the role's grants are the guarantee. Create a dedicated role per environment and enroll *that*.

### PostgreSQL

```sql
CREATE ROLE dbcrust_agent LOGIN PASSWORD '…'
  NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;

-- Belt and braces: every transaction of this role is read-only, and bounded
ALTER ROLE dbcrust_agent SET default_transaction_read_only = on;
ALTER ROLE dbcrust_agent SET statement_timeout = '30s';
ALTER ROLE dbcrust_agent SET idle_in_transaction_session_timeout = '30s';

GRANT CONNECT ON DATABASE app TO dbcrust_agent;
GRANT USAGE   ON SCHEMA public TO dbcrust_agent;
GRANT SELECT  ON ALL TABLES IN SCHEMA public TO dbcrust_agent;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO dbcrust_agent;
```

Grant `SELECT` on specific tables (or columns, or views that hide sensitive columns) rather than the whole schema wherever you can. Row-level security applies to the role like to any other. On PostgreSQL 14 and older, also `REVOKE CREATE ON SCHEMA public FROM PUBLIC`.

### MySQL / MariaDB

```sql
CREATE USER 'dbcrust_agent'@'10.0.%' IDENTIFIED BY '…' WITH MAX_USER_CONNECTIONS 5;
GRANT SELECT ON app.* TO 'dbcrust_agent'@'10.0.%';
```

Restrict the host part to where the broker runs. Grant `SELECT` on individual tables when the whole schema is too much.

### SQLite

SQLite has no roles: the file's permissions are the boundary. Point the alias at a **copy or read replica** the service user can read but not write, not at your live database file. (On Linux the systemd unit sets `ProtectHome=yes`: put the file outside `/home`.)

### ClickHouse

```sql
CREATE USER dbcrust_agent IDENTIFIED WITH sha256_password BY '…' SETTINGS readonly = 1;
GRANT SELECT ON app.* TO dbcrust_agent;
```

`readonly = 1` (via the user's settings or a profile) makes the server itself refuse writes and refuse changing settings.

## Managing policy

```bash
dbcrust broker list                                   # aliases and their limits; never credentials
dbcrust broker policy prod --max-rows 100 --timeout 10 --max-bytes 1000000
dbcrust broker policy prod --read-only                # downgrade a read-write alias
dbcrust broker revoke prod                            # delete the alias and its stored credential
```

(Add `--mode service` and run through `sudo /usr/local/libexec/dbcrust-broker …` in service mode.)

- **`policy` only tightens.** Lowering limits and switching to read-only need no proof; raising a limit or granting writes does not exist as a `policy` operation.
- **To loosen, re-enroll.** `broker enroll <alias> <url> --max-rows 5000` asks for the password again. Knowing the secret is the owner's proof; the same applies to changing the target.
- **Writes are opt-in per alias**: `broker enroll scratch sqlite:///tmp/scratch.db --allow-writes` lets agents run `INSERT`/`UPDATE`/`DELETE`. There is no per-write approval in this version.
- **DDL, DCL and administration are always refused**, writes allowed or not: `CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `GRANT`, `REVOKE`, `COPY … PROGRAM`, `VACUUM`, `ATTACH`, `SET ROLE`, writing `PRAGMA`s, `SELECT … INTO`, and so on. A client can ask for lower limits per request, never higher ones.

## The audit log

| Mode | Location |
|------|----------|
| user | `~/.config/dbcrust/broker/audit.jsonl` |
| service, macOS | `/Library/Application Support/dbcrust-broker/audit.jsonl` |
| service, Linux | `/var/lib/dbcrust-broker/audit.jsonl` |

One JSON object per line, mode 0600 (read it with `sudo` in service mode):

```json
{"ts":"2026-10-15T09:21:07Z","seq":41,"phase":"end","client_label":"claude-code","peer_uid":501,"peer_pid":8123,"alias":"prod","op":"query","decision":"allowed","code":null,"sql_sha256":"9f2c…","sql_len":58,"rows":120,"bytes":9310,"truncated":false,"duration_ms":14}
```

- A request that will touch the database writes a `phase: "start"` line *before* executing and an `end` line with rows, bytes and duration afterwards; refusals write a single `end` line with `decision: "refused"` and the `code`.
- SQL text, parameters and results are never written: only a SHA-256 and the length, so a statement can be matched to a known one without the log becoming a second copy of your data.
- **Fail closed:** if the start line cannot be written, the request is refused.
- Rotation: `audit.jsonl` plus four rotated files, 10 MiB each.

## MCP setup

`dbcrust-agent mcp` is a stdio MCP server exposing five tools: `list_connections`, `list_tables`, `describe_table`, `query` and `explain`. It supports MCP protocol versions `2025-06-18` and `2025-03-26`. Results come back as JSON text plus `structuredContent`; errors have `isError: true` and one of the codes in the [exit-code table](#one-shot-cli).

If the broker is at a non-default path, give the client the socket via `DBCRUST_BROKER_SOCKET` (a path — never a URL or password).

### Claude Code

```bash
claude mcp add dbcrust -- dbcrust-agent mcp                   # this project, just you (default "local" scope)
claude mcp add --scope project dbcrust -- dbcrust-agent mcp   # writes .mcp.json, shared with the team
claude mcp add --scope user dbcrust -- dbcrust-agent mcp      # all your projects
claude mcp add --env DBCRUST_BROKER_SOCKET=/path/broker.sock dbcrust -- dbcrust-agent mcp
claude mcp list
```

### Codex

`~/.codex/config.toml`:

```toml
[mcp_servers.dbcrust]
command = "dbcrust-agent"
args = ["mcp"]
# env = { DBCRUST_BROKER_SOCKET = "/path/broker.sock" }
```

### Cursor

`~/.cursor/mcp.json` (all projects) or `.cursor/mcp.json` (one project):

```json
{
  "mcpServers": {
    "dbcrust": { "command": "dbcrust-agent", "args": ["mcp"] }
  }
}
```

### Any other MCP client

Register a stdio server whose command is `dbcrust-agent` with the single argument `mcp`. Nothing else is needed: no URL, no token, no environment. `dbcrust-agent doctor` checks the socket, the handshake and the alias count from a terminal.

## One-shot CLI

Agents without MCP (or scripts) use the same broker from the shell:

```bash
dbcrust-agent list                                     # aliases, engine, access, limits
dbcrust-agent tables prod
dbcrust-agent describe prod public.users
dbcrust-agent query prod -c 'SELECT * FROM users' -o json --max-rows 50 --timeout 10
dbcrust-agent explain prod -c 'SELECT * FROM users WHERE id = 1'
dbcrust-agent doctor
```

`-o json` prints one single-line `{"columns":[…],"rows":[[…]],"row_count":N,"truncated":false}` envelope; errors go to stderr as `{"error":{"code":"…","message":"…"}}`.

| Exit | Meaning | Codes |
|---|---|---|
| 0 | success | |
| 1 | the statement failed | `query_error`, `unsupported_engine`, `internal` |
| 2 | usage, protocol mismatch, unsupported platform | `protocol_error`, `unsupported_version` |
| 3 | no connection to the broker, or the broker is locked | `connection_error`, `broker_locked` |
| 4 | refused by the read-only / DDL rules | `read_only_violation`, `ddl_refused`, `multi_statement_refused` |
| 5 | refused by policy or unknown alias | `denied`, `unknown_alias`, `limit_exceeded` |
| 124 | statement timed out | `timeout` |

## Troubleshooting

**`no DBCrust broker socket found`**: nothing is listening where the client looked. The error lists the paths it tried (service socket first, then the user socket). Check `dbcrust broker status --mode service` (or without `--mode` for user mode). macOS service log: `/Library/Logs/dbcrust-broker.log`; Linux: `journalctl -u dbcrust-broker`.

**Permission denied connecting to the socket / "not in dbcrust-agents"**: your user is not in the `dbcrust-agents` group, or was added after this login session started. `dbcrust broker status --mode service` shows both. Add yourself (`sudo dseditgroup -o edit -a "$USER" -t user dbcrust-agents` on macOS, `sudo usermod -aG dbcrust-agents "$USER"` on Linux) and **log out and back in**; group changes do not reach existing sessions. Processes started from an already-running IDE or terminal keep the old groups.

**`broker_locked`**: the broker cannot open its store. User mode with the Linux encrypted-file backend: run `dbcrust secrets unlock` and restart `dbcrust broker serve`. Service mode: the key file in the state directory is missing or unreadable by the service user; `broker status --mode service` shows permission problems.

**`refusing to run the service-mode broker from an untrusted binary`**: you ran `sudo dbcrust broker …` from a user-writable location. Use `sudo /usr/local/libexec/dbcrust-broker broker … --mode service`. If `install-service` itself reports that the installed binary would not be trusted, a parent directory (commonly `/usr/local` on an Intel Mac with Homebrew) is owned by a normal user: fix its ownership first.

**macOS keychain prompt after an upgrade (user mode)**: the keychain trusts the specific `dbcrust` binary that stored the item; a rebuilt or upgraded binary has a new identity and the first access asks again. Choose *Always Allow*. Service mode does not use the keychain.

**`unknown_alias` / `denied`**: the alias is not enrolled (see `dbcrust-agent list`), or the request exceeds the alias's policy. Refusals such as `read_only_violation` and `ddl_refused` are final: ask the database owner, don't retry with a rewritten statement.

**Enrolled SQLite file not found (Linux service)**: the unit sets `ProtectHome=yes` and the service user has no access to home directories; put the database file elsewhere or use user mode.

## Related

- [Secret storage](/dbcrust/user-guide/secret-storage/): where DBCrust itself keeps passwords and keys.
- [AI agents overview](/dbcrust/ai-agents/overview/) and [safety & guardrails](/dbcrust/ai-agents/safety/): the direct one-shot CLI for connections you do not need to protect.
- `dbcrust agents`: the guide agents read themselves; `crates/agent/README.md` and `crates/agent-protocol/PROTOCOL.md` in the repository for the client and the wire protocol.

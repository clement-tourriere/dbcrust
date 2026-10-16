---
title: "libSQL & Turso"
description: Connect to Turso Cloud and self-hosted sqld over HTTP (Hrana) with TLS by default, the auth token in the secret store, SQLite catalog browsing, transactions and a --read-only guard.
---

# libSQL & Turso

DBCrust talks to a remote **libSQL** server — [Turso](https://turso.tech) Cloud or a self-hosted
`sqld` — over **Hrana on HTTP** (JSON, the `/v2/pipeline` endpoint). The server is SQLite, so you
type SQLite SQL; results come back as tables you can sort, export with `-o json` / `-o csv`, or
pipe.

```sh
dbcrust 'libsql://my-db-myorg.turso.io?authToken=eyJhbGciOi…'
dbcrust 'turso://my-db-myorg.turso.io' --read-only -c 'SELECT * FROM users LIMIT 10'
dbcrust 'libsql+http://localhost:8080'          # a local sqld
```

libSQL joined the **default build** (`bundled`); the `standard` preset does not contain it. It
adds no crate: the client is a few hundred lines on the `reqwest` DBCrust already ships, with no
libSQL SDK. See [Build features](/user-guide/development/#build-features).

Only **remote** databases are in scope here. A local SQLite file is `sqlite:///path/to.db`;
embedded replicas and sync are not supported.

## Connecting

```text
libsql://host[:port][/namespace][?options]        # HTTPS (default port 443)
turso://…                                         # same thing
libsql+http://host[:port][/namespace][?options]   # plain HTTP (default port 8080)
```

| Part | Meaning |
|---|---|
| `host`, `port` | a Turso database (`my-db-myorg.turso.io`) or any host in front of `sqld` |
| auth token | the credential, a JWT. Give it as `?authToken=…` or as the URL password (`libsql://:TOKEN@host`) — or leave it out and DBCrust asks when the server answers 401 |
| `/namespace` | optional, for `sqld` running with namespaces: sent as the `x-namespace` header (`?namespace=` works too). Empty for Turso Cloud |

| Option | Default | Effect |
|---|---|---|
| `tls` | `true` | `false` selects plain HTTP, same as the `libsql+http://` scheme. **Plain HTTP to a host that is not this machine is refused** unless the URL says `tls=false`: the token and every query would cross the network in clear text |

**The auth token is a secret.** DBCrust moves it out of the URL into the connection's password
slot, so it follows the same rules as every other password: it is saved to the
[secret store](/user-guide/secret-storage/) (`\savepass`, `session://`), **bound to the exact
host, port, namespace and transport** (a token stored for `https` is never offered to the same
host over plain HTTP), and it never appears in a saved session, a recent connection, a log line
or an error message (`authToken` is redacted wherever a URL is shown). Prefer the prompt or the
secret store to a token typed in a shell history.

Turso: create a token with `turso db tokens create my-db` (add `--read-only` for a token that
cannot write). `sqld`: `docker run -p 8080:8080 ghcr.io/tursodatabase/libsql-server:latest`
needs no token; with `SQLD_AUTH_JWT_KEY` set it wants a JWT.

## Running SQL

Each statement is one HTTP round trip and **stateless**: DBCrust sends `execute` + `close` in one
pipeline, so nothing is left open on the server. A transaction is the exception: `BEGIN` keeps the
server-side stream open and DBCrust carries the stream's *baton* through `COMMIT` / `END` /
`ROLLBACK`, so this works as in `sqlite3`:

```sql
BEGIN;
DELETE FROM sessions WHERE expires < unixepoch();
SELECT count(*) FROM sessions;   -- sees the delete
ROLLBACK;
```

The server expires an idle stream after a short while. If it does, the next statement fails with
the server's message and *"the open transaction was lost"* — the server has already rolled it
back; start again with `BEGIN`.

- Values keep their SQLite type: integers exact (64-bit), floats with a fractional part
  (`3.0`), text, blobs as `\x` + hex, `NULL` apart from `''` inside DBCrust (the CLI's text and
  JSON output print it as an empty cell, like the other SQL engines).
- One statement per call, as Hrana wants: the CLI and `-f` already split scripts on `;`.
- There is no `REGEXP` on the server, so the `~` operators the SQLite driver translates are not
  translated here.
- The server's per-statement stats (`rows_read`, `rows_written`, duration — what Turso bills)
  are logged at debug level.

### Plans: `EXPLAIN`

`EXPLAIN` runs `EXPLAIN QUERY PLAN` and prints the steps as an indented tree; the GUI draws it
like a SQLite plan.

## Browsing

`\dt` lists tables and views (`sqlite_*` and the replication bookkeeping tables are hidden),
`\d table` shows columns (type, `NOT NULL`, default), indexes (with their columns), foreign keys
and the tables that reference this one; `\l` lists attached databases (`main`). Completion uses
the same catalog. `\d` costs two round trips however many indexes the table has.

## Read-only

`--read-only` (and the AI agent) accepts a single `SELECT`, `WITH … SELECT`, `EXPLAIN` or an
**introspection** `PRAGMA` (`table_info`, `index_list`, `foreign_key_list`, `user_version`, …) and
refuses everything else — including `ATTACH`/`DETACH`, `VACUUM`, `load_extension()`, `PRAGMA`
assignments and the `PRAGMA`s that act (`wal_checkpoint`, `incremental_vacuum`, `optimize`).

Then, for every statement, DBCrust asks the **server** (Hrana `describe`) whether SQLite itself
considers it read-only (`sqlite3_stmt_readonly`) and refuses anything that is not — before it is
ever executed. That is the SQLite parser's answer, not a heuristic, but it is still the
client's guard: the only hard limit is the **token**. Use a read-only token
(`turso db tokens create my-db --read-only`, or a `sqld` JWT with the `ro` access claim) for
anything an agent or a reporting job touches. `\serverinfo` shows what the token says about
itself (`token_access_claim`) — an unverified claim, because a client cannot prove that the
server checks it.

The [agent broker](/user-guide/agent-connections/) refuses to enroll libSQL
(`unsupported_engine`: *"libSQL enrollment for agents is not supported yet: a read-only token is
the only server-enforced limit and cannot be proven from the client"*). Proving it would need a
write, and a `sqld` without authentication ignores a token's claims altogether.

## Limits, timeouts and cancellation

- **Timeout**: `query_timeout_seconds` bounds each request (300 s otherwise); a timed-out request
  reports *"Query timed out after N seconds"*.
- **Cancel**: Hrana has no cancel request. Ctrl-C drops the HTTP request and the CLI returns
  immediately; the server finishes or abandons the statement on its own.
- **Rows**: `/v2/pipeline` returns a whole result, so a huge `SELECT` is transferred before it is
  shown. Keep the default `LIMIT` on and select only the columns you need — Turso bills rows read.
- **Errors**: 401/403 *"authentication failed"* (and a prompt for the token), 404 *"does not
  serve /v2/pipeline"* (not a libSQL endpoint, or an unknown namespace), 429 with the retry
  delay, 5xx with the server's message. A server-supplied `base_url` for a pinned stream is
  followed only on the same scheme, never from HTTPS to HTTP.

## Not supported yet

Local files and embedded replicas, the `libsql` crate's sync, interactive cursors
(`/v3/cursor` streaming for early stop), Turso's remote-encryption keys, the Platform API
(creating databases and tokens), and agent enrollment (above). The wire code is checked against
a fake Hrana server in the test suite and, when `LIBSQL_URL` is set, against a real `sqld` or
Turso database (`cargo test -p dbcrust-driver-libsql live_server`).

---
title: "Amazon Redshift"
description: Connect to Amazon Redshift (provisioned and Serverless) with TLS by default, SVV_* catalog browsing, Spectrum schemas, text EXPLAIN plans and a --read-only guard.
---

# Amazon Redshift

DBCrust connects to Amazon Redshift (provisioned clusters and Serverless workgroups) over the
PostgreSQL wire protocol with **user and password** authentication. You type Redshift SQL;
results come back as tables you can sort, export with `-o json` / `-o csv`, or pipe.

```sh
dbcrust 'redshift://awsuser:pass@my-cluster.abc123.eu-west-1.redshift.amazonaws.com:5439/dev'
dbcrust 'redshift://analyst@wg.123456789012.eu-west-1.redshift-serverless.amazonaws.com/dev' \
  --read-only -c 'SELECT * FROM sales.orders LIMIT 10'
```

Redshift is part of the **default build** (`bundled`) and not of the `standard` preset. It adds
no crate: it uses the same `sqlx` PostgreSQL stack as the PostgreSQL driver, but its own client,
because Redshift is a PostgreSQL 8.0.2 descendant and a stock PostgreSQL client leans on things
it does not have (see [How it differs from PostgreSQL](#how-it-differs-from-postgresql)). See
[Build features](/user-guide/development/#build-features).

## Connecting

```text
redshift://[user[:password]@]host[:port][/database][?options]
redshift+tls://…                   # same thing, and refuses sslmode=disable|allow|prefer
```

| Part | Meaning |
|---|---|
| `host`, `port` | default port `5439` (the cluster endpoint, or the Serverless workgroup endpoint) |
| `user`, `password` | a **database user**. The password is prompted for and saved to the secret store when the server rejects the login — same flow as the other engines (`\savepass`, `session://`). Percent-encode special characters (`p%40ss`) |
| `/database` | the database to open; empty opens `dev`, the default database of a new cluster. Redshift binds a connection to one database, so `\c other` opens a new connection |

Options in the query string (case-insensitive):

| Option | Default | Effect |
|---|---|---|
| `sslmode` | `require` | `require`: TLS, certificate **not** verified (like libpq); `verify-ca` / `verify-full`: certificate chain (and host name) checked against the bundled web roots, which include Amazon Root CA 1–4, or against `sslrootcert`; `disable` / `allow` / `prefer`: for an SSH tunnel or a local test endpoint only |
| `sslrootcert` | – | path of a PEM file holding the CA to trust (for `verify-ca` / `verify-full`) |
| `application_name` | `dbcrust` | shows up in `STV_SESSIONS` / `SVL_STATEMENTTEXT` |

Unlike PostgreSQL, the default is TLS, never "try TLS, fall back to plain": a cluster that does
not set `require_ssl` would otherwise receive the password in clear text. A saved session keeps
the host, port, user, database and `sslmode`; the password stays in the secret store **bound to
that exact host, port, database, user and TLS mode**, so it is never offered to a connection that
stopped requiring TLS. Reaching a cluster in a private VPC through a bastion works with the
[SSH tunnel patterns](/advanced/ssh-tunneling/) like any other engine.

**Not supported yet:** IAM authentication (`GetClusterCredentials`, Serverless
`GetCredentials`, IAM identity-center / federated sign-in) and the Redshift Data API. Use a
database user and password, or an SSH tunnel to a cluster whose endpoint is not public. They
are planned together with the other AWS connectors.

## Running SQL

Statements run on **one pinned session**, so `SET`, temp tables and `BEGIN … COMMIT` behave like
in `psql`. DBCrust uses the PostgreSQL *simple* query protocol only: values reach the screen
as the server printed them, which is also why every Redshift type works, new ones included.

- A string with several statements (`SET query_group TO 'etl'; SELECT …`) shows the first
  result set that returned rows.
- `SUPER` values print as JSON text, `VARBYTE` and `GEOMETRY` / `GEOGRAPHY` as hex, `TIMETZ`,
  `TIMESTAMPTZ`, `NUMERIC` and the rest exactly as Redshift formats them. SQL `NULL` is kept
  apart from the empty string inside DBCrust (the GUI shows it); the CLI's text and JSON output
  print it as an empty cell, like the other SQL engines.
- Ctrl-C and `query_timeout_seconds` cancel on the server (`pg_cancel_backend` from a second
  connection); the session survives. A broker-style timeout is also set as `statement_timeout`.

### Plans: `EXPLAIN`

Redshift prints text plans only (`XN Seq Scan … `, `DS_BCAST_INNER`…). `EXPLAIN SELECT …` and the
explain mode (`\e`) show them one line per row; there is no JSON plan, no
`EXPLAIN ANALYZE` and no plan visualizer. For a query that already ran, read `SVL_QUERY_REPORT`
or `SYS_QUERY_DETAIL`.

## How it differs from PostgreSQL

| | PostgreSQL | Redshift (DBCrust) |
|---|---|---|
| Protocol | extended (prepared) | simple queries only; no type lookups in `pg_type` |
| Startup | `options=-c …` | none (`SET` after connect) |
| Catalog | `pg_catalog`, `information_schema` | `SVV_TABLES`, `SVV_COLUMNS`, `SVV_TABLE_INFO`, `SVV_EXTERNAL_SCHEMAS`; `PG_TABLE_DEF` for encodings and the sort key |
| Indexes | yes | none: `\d` shows `DISTSTYLE` and `SORTKEY` where PostgreSQL shows indexes |
| Constraints | enforced | primary / unique / foreign keys are informational and listed as such |
| Arrays, JSONB, extensions | yes | no: `SUPER`, `LISTAGG`, `JSON_EXTRACT_PATH_TEXT` instead |
| `EXPLAIN` | JSON | text |

## Browsing

`\l` lists databases, `\dt` tables, views and late-binding views of every schema (Spectrum /
external schemas included), `\d table` the columns with their types, defaults and **encodings**
(`character varying(256) ENCODE lzo`), the distribution style and sort key, and any
informational keys. Completion is fed from the same views. `\dt` outside the `public` schema
shows `schema.table`.

`\d` reads `PG_TABLE_DEF` for the encodings and sort key, and Redshift only fills that view
for schemas on the `search_path`: for a table elsewhere the encodings are left out (the
distribution style still comes from `SVV_TABLE_INFO`). `SET search_path TO sales, public;` if
you want them.

## Read-only

`--read-only` (and the AI agent) accepts a single `SELECT`, `WITH … SELECT`, `SHOW` or
`EXPLAIN`, and refuses everything else: DML, DDL, `COPY` and `UNLOAD` (they read and write S3),
`CREATE EXTERNAL …`, `CALL`, `VACUUM`, `ANALYZE`, `GRANT`/`REVOKE`, `ALTER DATASHARE`,
`SET SESSION AUTHORIZATION`, `CANCEL`/`ABORT`, `PREPARE`/`EXECUTE`, `SELECT … INTO`,
`pg_terminate_backend` / `pg_cancel_backend`, and several statements at once.

On top of that guard every statement runs inside `BEGIN READ ONLY` … `ROLLBACK`. That is the
transaction mode Redshift documents for `BEGIN`, but it is **per transaction**: Redshift has no
session-level `default_transaction_read_only`, and DBCrust has not been able to prove from the
outside that a cluster enforces it. Treat it as a second line of defence, not a guarantee. The
real limit is the database user: give an agent or a reporting login `GRANT SELECT` on what it
needs and nothing else.

```sql
CREATE USER reporting PASSWORD 'Report1ngPassw0rd';
GRANT USAGE ON SCHEMA sales TO reporting;
GRANT SELECT ON ALL TABLES IN SCHEMA sales TO reporting;
```

The [agent broker](/user-guide/agent-connections/) refuses to enroll Redshift
(`unsupported_engine`: *"Redshift enrollment for agents is not supported yet: the server has no
session-level read-only mode to enforce"*). A broker alias promises a server-enforced limit, and
for Redshift only the user's grants provide one.

## Not supported yet

IAM and Data API access (above); `UNLOAD` / `COPY` helpers (they run as ordinary statements
when not `--read-only`); result paging by cursor (large results are read whole, so keep
`LIMIT` on, or use the default auto-limit). The Redshift-specific catalog queries are checked
against a fake server in the test suite and, when `REDSHIFT_URL` is set, against a real
cluster (`cargo test -p dbcrust-driver-redshift live_cluster`).

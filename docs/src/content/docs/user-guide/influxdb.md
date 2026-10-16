---
title: "InfluxDB"
description: Query InfluxDB v1 (InfluxQL), v2 (Flux) and v3 (SQL) over HTTP, browse measurements, and use --read-only knowing what the server does and does not enforce.
---

# InfluxDB

DBCrust talks to InfluxDB over its HTTP APIs. The URL scheme picks the API generation, and with it
the language you type:

| Scheme | Server | You type | Endpoint |
|---|---|---|---|
| `influxdb://` | v1 (also v2 with its 1.x-compatible API) | InfluxQL | `GET /query` |
| `influxdb2://` | v2 | Flux | `POST /api/v2/query` |
| `influxdb3://` | v3 (Core, Enterprise) | SQL | `POST /api/v3/query_sql` |

A trailing `s` (`influxdbs://`, `influxdb2s://`, `influxdb3s://`) or `+https` means TLS.

```sh
dbcrust influxdb://localhost:8086/telemetry -c 'SELECT mean(usage) FROM cpu WHERE time > now() - 1h GROUP BY time(5m), host'
dbcrust 'influxdb2://:MY_TOKEN@localhost:8086/telemetry?org=acme' -c 'from(bucket: "telemetry") |> range(start: -1h) |> limit(n: 10)'
dbcrust 'influxdb3://:MY_TOKEN@localhost:8181/mydb' -c 'SELECT * FROM cpu WHERE time >= now() - interval '"'"'1 hour'"'"' LIMIT 10'
```

InfluxDB joined the **default build** (`bundled`) in this release; the `standard` preset does not
contain it. It adds no third-party crate (a thin HTTP client on the `reqwest` and `csv-core`
already in the tree), see [Build features](/user-guide/development/#build-features).

## Connecting

```text
influxdb://[user[:password]]@host[:port][/database][?options]            # v1, InfluxQL
influxdb2://[:token]@host[:port][/bucket]?org=<organization>[&options]    # v2, Flux
influxdb3://[:token]@host[:port][/database][?options]                     # v3, SQL
```

| Part | Meaning |
|---|---|
| `host`, `port` | default `localhost`; port **8086** for v1 and v2, **8181** for v3 (made explicit when the URL is read) |
| `/database` | the v1 database, the v2 **bucket**, the v3 database. `?db=`, `?database=` and `?bucket=` also work |
| `user`, `password` | v1 login (HTTP Basic). Sent as a header, never in the query string |
| API token | v2 and v3: in the password slot of the URL (`:TOKEN@`) or as `?token=`. DBCrust moves it into the password slot, so it is stored, prompted for and redacted exactly like a password |
| `org` | **required for v2**: the organization name |
| `?version=1\|2\|3` | picks the generation on the generic `influxdb://` scheme (`influxdb://host/b?version=2&org=acme`) |
| `?language=influxql` | InfluxQL on a v2 server (its 1.x-compatible API, which needs a [DBRP mapping](https://docs.influxdata.com/influxdb/v2/api-guide/influxdb/1x/dbrp/)) or on a v3 server (`query_influxql`) |
| `?rp=` | v1/v2 InfluxQL retention policy |
| `?epoch=` | v1/v2 InfluxQL timestamps as integers (`ns`, `u`, `ms`, `s`, `m`, `h`) instead of RFC 3339 text |

A v1 URL with only a token (`influxdb://host/db?token=…`) sends it as `Authorization: Token …`,
which a v2 server's 1.x-compatible API accepts.

The token never appears in `Debug` output, error messages, connection banners, recent
connections or the saved-session file. See [Sessions and secrets](#sessions-and-secrets).

### TLS

`influxdbs://` (or `influxdb+https://`, or `?tls=true` / `?ssl=true`) enables TLS with certificate
and hostname verification against the bundled web PKI roots. Two options adjust the trust:

| Option | Effect |
|---|---|
| `ca_cert=/path/ca.pem` | trust this CA in addition (private CAs, self-signed servers with a CA file) |
| `verify_certs=false` | skip certificate verification, with a warning in the log. A saved token is never handed to a connection that stopped verifying |

InfluxDB Cloud URLs need their port: `influxdb2s://us-east-1-1.aws.cloud2.influxdata.com:443/bucket?org=acme`.

### Docker

`docker://my-influx/telemetry` finds `influxdb` images (not Telegraf, Chronograf, Kapacitor or
exporters) and reads the credentials from the container environment (`INFLUXDB_ADMIN_USER` /
`INFLUXDB_ADMIN_PASSWORD` for v1 images, `DOCKER_INFLUXDB_INIT_ADMIN_TOKEN` for v2). A container
does not say which generation it runs: add `?version=2&org=acme` (or `3`).

## Running queries

Whatever you type is the query in the connection's language, one statement at a time:

- **InfluxQL (v1)**: `SELECT`, `SHOW`. DBCrust adds `LIMIT` to a plain `SELECT` like for SQL
  engines (the `default_limit` setting), except when the statement already has `OFFSET`,
  `SLIMIT`, `SOFFSET` or `tz()`.
- **Flux (v2)**: a whole program, several lines are fine. DBCrust never splits it on `;`. The
  server rejects an unbounded `from(bucket:)`, so every query needs a `range()`.
- **SQL (v3)**: DataFusion SQL; `EXPLAIN SELECT …` shows the logical and physical plans.

### How a reply becomes a table

| Generation | Reply | Table |
|---|---|---|
| v1 | JSON series `{name, tags, columns, values}`, streamed in chunks | one table. A `name` column appears when the series have several names, a column per `GROUP BY` tag comes next, then the series columns. A statement list shows the first non-empty result and a notice for the rest |
| v2 | annotated CSV: `#datatype`, `#group`, `#default`, header, rows; tables with other schemas follow after a blank line | one table, columns in first-seen order and `NULL` where a block has no such column. The `result` column is hidden while it only says `_result`, and `table` while the reply has a single table. An empty cell is `NULL` unless the `#default` row gives it a value (annotated CSV cannot tell `""` from null) |
| v3 | CSV | one table, an empty cell is `NULL` |

Everything is text: numbers keep the server's spelling, timestamps are RFC 3339 (v1 without
`epoch=`, v2) or the server's own format (v3). A Flux error table (`error`, `reference`) becomes
the query error.

### Limits, timeouts and cancellation

- The reply is read **while it arrives**. At `max_rows` rows (default 10 000, `0` = unlimited) or
  `max_bytes` bytes (default 64 MiB) the connection is dropped, which makes the server stop
  producing, and a warning says the result was cut. A JSON document cut by `max_bytes` is lost as
  a whole; v1 sends documents of at most 5 000 rows, so the rows before it are kept.
- The `query_timeout` setting and Ctrl-C abandon a running request by closing the connection. The
  server finishes or abandons the query on its own (`query-timeout` in the v1 `[coordinator]`
  section, `--query-timeout` in v2); there is no per-request server timeout to set.
- There is no EXPLAIN on v1 and v2 (only v3 has a planner).

## Browsing

| Command | v1 | v2 | v3 |
|---|---|---|---|
| `\l` | `SHOW DATABASES` | `buckets()` | the database list |
| `\dt` | `SHOW MEASUREMENTS` | `schema.measurements()` | `information_schema.tables` |
| `\d <measurement>` | `time`, tag keys, field keys with types | `time`, tag keys, field names | columns; dictionary columns are tags |
| `\c <name>` | checks the name against the list, switches | same (a token that cannot list buckets is taken at its word) | same |

Flux's `schema` functions are time-bounded: they look back **30 days** unless you set
`schema_window` (a Flux duration, e.g. `?schema_window=-1y`). A measurement with no data in the
window does not show up in `\dt` and `\d`.

Measurements are the "tables" of the completion and AI schema context; tag and field keys are the
columns.

## Read-only

`--read-only` (and the AI agent) accept only reads, judged by a lexer in the language of the
connection (strings, regexes, comments and quoted names never count):

| Language | Allowed | Refused |
|---|---|---|
| InfluxQL | `SELECT`, `SHOW`, `EXPLAIN` of one | `SELECT … INTO` (it writes), `DROP`, `DELETE`, `CREATE`, `ALTER`, `GRANT`, `REVOKE`, `KILL` and anything else, in any statement of a list |
| SQL (v3) | the above plus `WITH … SELECT` and `DESCRIBE` | writes, DDL, `SET`, `COPY` |
| Flux | queries built from allow-listed imports (`array`, `date`, `dict`, `experimental`, `json`, `math`, `regexp`, `strings`, `timezone`, `types`, `universe`, `influxdata/influxdb`, `…/schema`, `…/v1`, `interpolate`) | every other import (`http`, `experimental/http`, `sql`, `socket`, `kafka`, `mqtt`, `csv`, `influxdata/influxdb/secrets`, `…/tasks`, `…/monitor`, the notification endpoints…), `to()` / `experimental.to()` / `influxdb.wideTo()`, `from(host:, token:, url:)` (a read of another server), `option task`, `builtin` |

Statements that are not reads never reach the server in read-only mode.

:::caution[The server does not back this up]
Unlike PostgreSQL's read-only transactions, **InfluxDB 1.x does not enforce read-only for `GET`**:
`DROP MEASUREMENT` or `SELECT … INTO` sent as `GET /query` still run, and the reply only carries
a warning (*"deprecated use of … in a read only context, please use a POST request instead"*;
checked on 1.8). DBCrust sends reads as `GET` and anything else as `POST` as the API documents,
and shows that warning as a notice if a statement it called a read triggers it. But the real
limit is the credential: a v1 user with `READ` privileges only (`GRANT READ ON telemetry TO
reader`), or a v2/v3 API token with read permissions only. Flux has no read-only mode at all.
:::

Because of that, `dbcrust broker enroll` refuses InfluxDB (`unsupported_engine`: *"InfluxDB
enrollment for agents is not supported yet: use a read-only API token or a v1 user limited to
reads"*): the broker's classifier and row-limit wrapper are SQL-only and it cannot prove what an
InfluxDB credential may do. Give an agent a read-only credential and the plain `--read-only`
connection instead.

## Sessions and secrets

`\ss`, `session://` and recent connections work as for the other engines. What is saved is the
host, port, database/bucket and the options that name the target (`version`, `org`, `tls`,
`language`, `rp`…); the **token is stored in the secret store** (OS keychain or the passphrase
file), bound to the API generation, host, port, database, user, TLS, the certificate opt-out and
the organization. A stored token is therefore never handed to another organization, a different
API generation, a plain-HTTP variant or a connection that stopped verifying certificates.

## Not supported yet

- Writes through line protocol (`/write`, `/api/v2/write`): DBCrust is a query client.
- InfluxDB v2 management APIs (buckets, tokens, tasks), Flux `buckets()` is the only one used.
- Table previews of the desktop app on a v2 (Flux) connection: they send SQL-style `SELECT` and
  fail there. Use `?language=influxql` or type Flux in the editor.
- Automatic generation detection: the scheme (or `?version=`) decides.
- Broker enrollment (see above).

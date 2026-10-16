---
title: "Amazon DynamoDB"
description: Read Amazon DynamoDB (and DynamoDB Local) with PartiQL SELECT and a few typed commands, with bounded reads, capacity reporting and no writes. Opt-in build.
---

# Amazon DynamoDB

DynamoDB has two read surfaces and DBCrust speaks both: **PartiQL** (`SELECT … FROM "Orders" WHERE …`)
and five **typed commands** (`tables`, `describe`, `scan`, `get`, `query`). Both only **read**: there is
no INSERT/UPDATE/DELETE, no put, no batch or transaction, in the REPL, in `-c`/`-f`, in `--read-only`
and for the AI assistant alike.

```sh
dbcrust dynamodb://eu-west-1 -c 'tables'
dbcrust 'dynamodb://eu-west-1/Orders?profile=prod-ro' -c 'query customer=acme --desc --limit 20'
dbcrust 'dynamodb://eu-west-1?profile=prod-ro' -c "SELECT * FROM \"Orders\" WHERE customer = 'acme'"
dbcrust 'dynamodb+http://localhost:8000?region=local' -c 'tables'     # DynamoDB Local
```

:::caution[Opt-in build, Rust 1.94.1]
DynamoDB is **not** part of the default (`bundled`) build, nor of `standard`. The AWS SDK needs
**Rust 1.94.1 or newer** to compile (the rest of DBCrust builds with 1.91). Build it with:

```sh
cargo install --path . --locked --features aws          # every AWS connector (S3, DynamoDB, CloudWatch Logs)
cargo build --features dynamodb                         # DynamoDB only
```

A default build answers *"Amazon DynamoDB support is not compiled into this build (enable the
`dynamodb` cargo feature, or install a full build)"*. See [Build features](/user-guide/development/#build-features).
:::

## Connecting

```text
dynamodb://[ACCESS_KEY[:SECRET_KEY]@][region][/table][?options]
dynamodb://[ACCESS_KEY[:SECRET_KEY]@]endpoint-host[:port][/table][?options]     # a custom endpoint over HTTPS
dynamodb+http://[ACCESS_KEY[:SECRET_KEY]@]host[:port][/table][?options]         # a custom endpoint over plain HTTP
ddb://…                                                                         # short alias
```

| Part | Meaning |
|---|---|
| `region` | `eu-west-1`, `us-gov-west-1`… A host shaped like a region is the region; any other host is a custom endpoint. Or leave it out and use `?region=`, a profile's region or `AWS_REGION` |
| `/table` | the default table of `scan`, `get`, `query`, `describe`. Leave it out to start without one; `\c TABLE` switches |
| `ACCESS_KEY:SECRET_KEY@` | static credentials. The secret key is the connection's **password**: it goes to the [secret store](/user-guide/secret-storage/), is bound to the endpoint, region and table it was saved for, and is redacted everywhere. With neither region nor endpoint host: `dynamodb://KEY:SECRET@?region=eu-west-1` |
| `?region=` | the region. Order: the URL host, `?region=`, the region of an explicit `?profile=`, then the environment (`AWS_REGION`) and the default profile |
| `?endpoint=` | a non-AWS endpoint: `https://host[:port]` or `http://host[:port]` (a bare `host:port` is HTTPS). It becomes the connection's host, port and TLS flag |
| `?profile=` | a named profile of `~/.aws/config` (SSO, assumed roles, keys, `credential_process`) |
| `?session_token=` | the session token of temporary static keys (a secret option, stored with the keys) |
| `?role_arn=`, `external_id=`, `session_name=` | assume a role on top of the credentials above |
| `?ca_cert=/path/ca.pem` | trust this CA in addition to the system roots |
| `?connect_timeout=`, `operation_timeout=`, `max_attempts=` | seconds to connect (default 10), seconds per operation (default: the query timeout), attempts (default 3) |
| `?consistent_read=true` | strongly consistent reads (twice the capacity of an eventually consistent read; not available on global indexes) |

Credentials are resolved exactly as for [S3](/user-guide/s3/#credentials): static keys, an explicit
`?profile=` (which wins over `AWS_ACCESS_KEY_ID`), or the AWS default chain (environment, shared config
and SSO, web identity, ECS, instance metadata). Contradictions are refused rather than resolved
silently. Without a region and with a custom endpoint, DBCrust signs for `us-east-1` (DynamoDB Local
and most emulators ignore it); against AWS itself a missing region is an error that says how to set one.

DBCrust always records the canonical spelling (recent connections, saved sessions):
`dynamodb+http://localhost:8000/Orders?region=us-east-1` becomes
`dynamodb://us-east-1/Orders?endpoint=http://localhost:8000`. It never contains the secret.

### DynamoDB Local and emulators

```sh
docker run -p 8000:8000 amazon/dynamodb-local
dbcrust 'dynamodb+http://local:local@localhost:8000?region=us-east-1' -c 'tables'
dbcrust 'dynamodb://us-east-1?endpoint=http://localhost:4566'          # LocalStack
```

DynamoDB Local accepts any credentials. Its PartiQL support is partial and it has no scan-forbidding
IAM conditions: test the exact statements against AWS before relying on them.

## Commands and PartiQL

A line is a **command** (the first word is the verb, case-insensitive) or a **PartiQL `SELECT`**.
`-f` files take one command per line; a `SELECT` may span lines and ends at its `;`. Blank lines and
`#`/`--` comments between statements are skipped. Arguments are shell-like words (`'single'` and
`"double"` quotes). `TABLE` may be left out when the connection names a default table.

| Command | DynamoDB call | Result |
|---|---|---|
| `tables [-n\|--limit N]` | ListTables | `table` |
| `describe [TABLE]` | DescribeTable | `property, value`: status, keys, key attribute types, billing, approximate item count and size (refreshed about every 6 hours), each global/local index with its keys and projection, stream, deletion protection, table class, ARN |
| `scan [TABLE] [--index I] [-n\|--limit N]` | Scan | the items; **reads every item it passes and is billed for it** |
| `get [TABLE] KEY=VALUE [SORTKEY=VALUE]` | GetItem | the one item, or no rows |
| `query [TABLE] KEY=VALUE [SORTKEY=VALUE \| SORTKEY^=PREFIX] [--index I] [-n\|--limit N] [--desc]` | Query | the items of one partition, optionally one sort key or a prefix of it (`begins_with`, string and binary sort keys) |
| `SELECT … FROM "Table"."Index" [WHERE …]` | ExecuteStatement | the items |
| `help` | none | the commands |

Key values are text. DBCrust runs a `DescribeTable` (cached) and converts them to the key attribute's
type: a string, a number (`ts=1760000000`, validated) or a binary (give it as base64). A key that is
not in the table's key schema, a missing sort key, or `^=` on a number is refused **before any read**,
with the table's key schema in the message.

```text
$ dbcrust 'dynamodb+http://local:local@localhost:8000/Orders?region=us-east-1' -c 'query customer=acme --desc'
customer | ts | addr             | tags      | total
---------+----+------------------+-----------+------
acme     |  3 | {"city":"Paris"} | ["a","b"] |   3.5
acme     |  2 | {"city":"Paris"} | ["a","b"] |   2.5
acme     |  1 | {"city":"Paris"} | ["a","b"] |   1.5
(3 rows)
```

### Items as rows

DynamoDB is schemaless, so the columns are the **union of the attribute names** of the items returned:
the table's key attributes first (and the index's, with `--index`), then every other name in the order
it is first seen. The attributes of one item have no order in DynamoDB, so the names an item adds are
taken alphabetically. More than **200** distinct names fold the rest into a trailing `_other` JSON cell.

| DynamoDB type | Cell |
|---|---|
| `S` | the string |
| `N` | the number **as the exact text DynamoDB sent**, never through a float |
| `B` | base64 |
| `BOOL` | `true` / `false` |
| `NULL` | SQL NULL (an attribute missing from an item is NULL too) |
| `SS`, `NS`, `BS` | a JSON array of strings, numbers (exact) or base64 strings |
| `L`, `M` | compact JSON (object keys sorted); nested numbers stay exact |

### Cost: every read is bounded and reported

DynamoDB bills by what a request **reads**, not by what it returns, and a `Limit` counts items
*evaluated*. So every request DBCrust sends carries a `Limit` that never exceeds the rows still wanted,
every loop has a page budget, and after each command the capacity it consumed is reported:

```text
INFO dbcrust_driver_dynamodb: DynamoDB: consumed 0.5 capacity units in 1 request for 1 item
```

- `scan` always warns (*"scan reads items without a key condition and is billed for everything it
  reads; `query` is the cheap read"*) and is bounded by `--limit`, or by `max_rows` (1000) when none is
  given. A result cut short says so.
- A PartiQL `SELECT` is bounded the same way (`Limit` per request, `max_rows`, `max_pages`). **A `SELECT`
  without a condition on the partition key is a full table scan**; PartiQL gives no way to tell before
  running it. Put the partition key in the `WHERE`, or use `query`. DBCrust does not append its automatic
  `LIMIT` to PartiQL (DynamoDB rejects the clause); `max_rows` is the cap.
- Notices and warnings go to the log (stderr), so result output stays parseable.

### Options (URL query)

| Option | Default | Meaning |
|---|---:|---|
| `max_rows` | 1000 (max 100 000) | items per result, and tables listed by `tables` |
| `max_pages` | 10 (max 1000) | requests per command |
| `scan_limit` | 1000 | tables `\dt` and completion list |
| `consistent_read` | false | strongly consistent reads |

## Browsing with backslash commands

| Command | Shows |
|---|---|
| `\l`, `\dt` | the account's tables in the region (`ListTables`) |
| `\c TABLE` | switch the default table (checked with DescribeTable) |
| `\d TABLE` | the key attributes (types and roles) and the primary key and indexes |

DynamoDB has no columns beyond the key attributes: nothing here samples items, because that costs
capacity. Tab completion offers the commands, `SELECT`, `--flags` and table names.

## Read-only by construction

The only PartiQL statement accepted is **one `SELECT`**. A lexer reads the statement through comments,
quoted identifiers and string literals, so `INSERT`, `UPDATE`, `DELETE`, `EXISTS(…)`, a second statement
after a `;` and a write hidden behind a comment are all refused, and `--read-only`, the AI assistant
and the driver itself share that one judge: what the guard accepts is exactly what runs. Writes are not
offered at all in this release (not even without `--read-only`).

That is a statement about the **client**: what the credentials may do is decided by **IAM**, which a
client cannot prove. For a hard limit give the key a read-only policy; to forbid full scans on the
server, add the `dynamodb:FullTableScan` condition on `PartiQLSelect`:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    { "Effect": "Allow",
      "Action": ["dynamodb:ListTables", "dynamodb:DescribeTable", "dynamodb:GetItem", "dynamodb:Query", "dynamodb:Scan"],
      "Resource": "*" },
    { "Effect": "Allow", "Action": "dynamodb:PartiQLSelect",
      "Resource": "arn:aws:dynamodb:eu-west-1:123456789012:table/Orders",
      "Condition": { "Bool": { "dynamodb:FullTableScan": "false" } } }
  ]
}
```

Connecting runs `ListTables` (or `DescribeTable` when the URL names a table). Being **denied** either
does not stop the connection (a policy that only grants `GetItem` and `Query` on one table still
works); an unknown table, bad or expired credentials or an unreachable endpoint do, and say why: the
DynamoDB error code, a hint, the request id.

The [credential broker](/user-guide/agent-connections/) **refuses** DynamoDB for now
(`unsupported_engine`): IAM cannot be proven read-only from the client, and an SDK client would inherit
the broker process' `AWS_*` environment and `~/.aws`.

## Sessions and secrets

Saved sessions and recent connections keep the table, endpoint, region, profile and the access key id;
the **secret access key and the session token go to the [secret store](/user-guide/secret-storage/)**,
bound to the endpoint host and port, the table, the access key id, the transport and the region. A key
saved for `eu-west-1` is never offered to `us-east-1`, to another table or endpoint, to a plain-HTTP
endpoint, or to S3. Errors, `Debug` output and logs mask keys, tokens and anything shaped like an
access key id (`AKIA…`, `ASIA…`).

## AI assistant

`??` can read a DynamoDB account: it is told the commands, the PartiQL form and the cost of scans, may
only run what `--read-only` accepts, and sees the same bounded results.

## Desktop app

The desktop app is built without the AWS connectors and does not offer DynamoDB in its connection
form. Use the CLI.

## What is not supported (yet)

- Writing in any form (PutItem, UpdateItem, DeleteItem, PartiQL INSERT/UPDATE/DELETE, batch and
  transactional APIs). They will come behind an explicit flag.
- `Scan` filter expressions, parallel scan segments, projections, `BatchGetItem`, DynamoDB Streams,
  PITR, backups and exports.
- Sampling items to describe a table's attributes.
- Agent enrollment through the broker.

## Troubleshooting

| Message | Cause |
|---|---|
| *Amazon DynamoDB support is not compiled into this build* | rebuild with `--features aws` (Rust ≥ 1.94.1) |
| *no AWS region: use dynamodb://eu-west-1, add ?region=…* | AWS needs a region; set it in the URL, `?region=`, `AWS_REGION` or a profile |
| *ResourceNotFoundException* | no such table in this region and account: `tables` lists them; check `?region=` and `?endpoint=` |
| *UnrecognizedClientException* | the access key id is unknown to this endpoint, or the session token is invalid |
| *InvalidSignatureException* | wrong secret key, or key issued for another endpoint or region |
| *ExpiredTokenException* | refresh the session (`aws sso login`, a new token) |
| *AccessDeniedException* | the policy lacks the action the command needs (`dynamodb:Query`, `GetItem`, `Scan`, `PartiQLSelect`, `DescribeTable`, `ListTables`) |
| *ProvisionedThroughputExceededException* / *ThrottlingException* | the table is throttled: wait, or lower the limit |
| *Unsupported clause: LIMIT* | PartiQL has no LIMIT; DBCrust does not add one, so the statement was sent to a client that does |
| *key X is a number; 'Y' is not one* | key values are converted to the key attribute's type |
| *an access key id needs its secret access key* | `dynamodb://AKIA…@region` without a secret and none saved; use `KEY:SECRET@`, a saved session, or `?profile=` |

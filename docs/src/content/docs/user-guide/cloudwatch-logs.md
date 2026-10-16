---
title: "Amazon CloudWatch Logs"
description: Query CloudWatch Logs with Logs Insights over a mandatory time window, read recent events, and see what a query scanned. Queries are stopped on Ctrl-C and on timeout. Opt-in build.
---

# Amazon CloudWatch Logs

A DBCrust "query" for CloudWatch Logs is a **Logs Insights** query over a **time window**, or one of
four typed commands (`groups`, `streams`, `tail`, `use`). Nothing writes: there is no `PutLogEvents`,
no group or stream management, no retention, export or subscription call, in the REPL, in `-c`/`-f`,
in `--read-only` and for the AI assistant alike.

```sh
dbcrust cloudwatch://eu-west-1 -c 'groups /aws/lambda/'
dbcrust 'cloudwatch://eu-west-1/aws/lambda/orders?profile=prod-ro' \
  -c 'fields @timestamp, @message | filter @message like /ERROR/ | sort @timestamp desc | limit 50'
dbcrust 'cloudwatch://eu-west-1/aws/lambda/orders' -c 'since 15m stats count(*) by bin(1m)'
```

:::caution[Opt-in build, Rust 1.94.1]
CloudWatch Logs is **not** part of the default (`bundled`) build, nor of `standard`. The AWS SDK needs
**Rust 1.94.1 or newer** to compile (the rest of DBCrust builds with 1.91). Build it with:

```sh
cargo install --path . --locked --features aws          # every AWS connector (S3, DynamoDB, CloudWatch Logs)
cargo build --features cloudwatch                       # CloudWatch Logs only
```

A default build answers *"Amazon CloudWatch Logs support is not compiled into this build (enable the
`cloudwatch` cargo feature, or install a full build)"*. See [Build features](/user-guide/development/#build-features).
:::

## Connecting

```text
cloudwatch://[ACCESS_KEY[:SECRET_KEY]@][region][/log-group-or-prefix][?options]
cloudwatch://[ACCESS_KEY[:SECRET_KEY]@]endpoint-host[:port][/…]          # a custom endpoint over HTTPS
cloudwatch+http://[ACCESS_KEY[:SECRET_KEY]@]host[:port][/…]              # a custom endpoint over plain HTTP
cwlogs://…                                                               # short alias
```

| Part | Meaning |
|---|---|
| `region` | `eu-west-1`, `us-gov-west-1`… A host shaped like a region is the region; any other host is a custom endpoint. Or leave it out and use `?region=`, a profile's region or `AWS_REGION` |
| `/log-group-or-prefix` | the log group(s) queries run on. AWS-managed groups start with `/aws/`, so `cloudwatch://eu-west-1/aws/lambda/orders` means `/aws/lambda/orders` (any other leading slash is written twice: `cloudwatch://eu-west-1//my-app`). If a group of exactly that name exists it is the only one selected; otherwise every group that **starts with** it (at most 50). Leave it out to start without a group and choose with `use` |
| `?groups=` | a comma-separated list of names, or `prefix*`, selected in addition |
| `ACCESS_KEY:SECRET_KEY@` | static credentials. The secret key is the connection's **password**: it goes to the [secret store](/user-guide/secret-storage/), is bound to the endpoint, region and log group it was saved for, and is redacted everywhere. With neither region nor endpoint host: `cloudwatch://KEY:SECRET@?region=eu-west-1` |
| `?region=`, `?profile=`, `?endpoint=`, `?session_token=`, `?role_arn=`, `?ca_cert=`, timeouts | exactly as for [DynamoDB](/user-guide/dynamodb/#connecting) and [S3](/user-guide/s3/#connecting) |
| `?since=` | the default time window of a query without one (default `1h`) |
| `?max_window=` | the widest window a query may ask for (default `7d`) |
| `?max_scan_gb=` | stop an Insights query that has scanned more than this many GiB (default: no budget) |
| `?query_timeout=` | seconds before a query is stopped (`0`: the service's own 60 minutes). Default: the configured `query_timeout_seconds`, **30 s**: raise it for long queries |

Credentials are resolved exactly as for [S3](/user-guide/s3/#credentials): static keys, an explicit
`?profile=`, or the AWS default chain. Without a region and with a custom endpoint, DBCrust signs for
`us-east-1`; against AWS itself a missing region is an error that says how to set one.

## Queries

Anything that is not a command is a **Logs Insights query** (the CloudWatch Logs Insights query
language). Its first command must be one of `fields`, `filter`, `filterIndex`, `stats`, `parse`, `sort`,
`limit`, `display`, `dedup`, `pattern`, `diff`, `unnest`, `anomaly`; anything else (`put`, `delete`, SQL…)
is refused before a request is made.

```text
[since DURATION | between TIME and TIME] [--group GROUP[,GROUP…]]  fields … | filter … | stats …
```

| Prefix | Meaning |
|---|---|
| `since 30m` | the last 30 minutes. `DURATION` is `90s`, `30m`, `2h`, `1d`, `1w` or a mix (`1h30m`) |
| `between 2026-10-16 08:00 and 2026-10-16 09:30` | an explicit window. `TIME` is RFC 3339 (`2026-10-16T08:00:00Z`, with an offset if you like), `YYYY-MM-DD HH:MM[:SS]` or `YYYY-MM-DD` (both UTC), or `now` |
| `--group /aws/lambda/a,/aws/lambda/b` | run this query on these groups (a name ending in `*` is a prefix) instead of the selected ones |

**Every query has a window.** Without a prefix it is the last hour (`?since=` changes that); there is
no query without one, and a window wider than `max_window` is refused before anything is sent.
`use /aws/lambda/orders,/aws/lambda/users*` changes the selected groups for the following queries
(at most 50, the service's limit per query).

```text
$ dbcrust 'cloudwatch://eu-west-1/aws/lambda/orders' -c 'since 1h fields @timestamp, @message | filter @message like /timeout/ | limit 3'
@timestamp              | @message
------------------------+-----------------------------------------
2026-10-16 08:01:12.311 | ERROR upstream timeout after 3000 ms
2026-10-16 07:58:40.002 | ERROR upstream timeout after 3000 ms
(2 rows)
INFO dbcrust_driver_cloudwatch: CloudWatch Logs: Logs Insights matched 2 records, scanned 184211 records (41.20 MiB) in 1 log group; window 1h (2026-10-16T07:30:00.000Z .. 2026-10-16T08:30:00.000Z)
```

Results are the fields the query outputs, in the order the first row has them (`fields` order), a
field a row does not have is NULL, and Insights' `@ptr` is hidden. A field name that occurs twice in a
row keeps both (`count(*)`, `count(*)_2`). Multi-line queries work in the REPL and in `-f` files: a
query ends at its `;`, at a blank line, or at the end of the file, and a command is one line.

### Cost: scanned bytes are the bill

Logs Insights charges **per GB scanned**. DBCrust therefore:

- sends every query with a mandatory window and at most 50 groups;
- sets the query's result `limit` to `max_rows` (default 1000, at most 10 000, the API's cap) and says
  when a result reached it (*"may be cut short: narrow the window or the filter"*);
- reports after each query what it did (a notice in the log, on stderr): records matched, records
  scanned, **bytes scanned**, the groups and the window;
- stops the query and fails with the numbers as soon as `bytesScanned` crosses `max_scan_gb`, when set.

### Queries are stopped, not abandoned

A query keeps scanning (and billing) on the server until it ends. DBCrust calls **`StopQuery`** on every
path that leaves one running: **Ctrl-C**, the **timeout**, a **scan budget**, and any failing poll. The
error says whether the stop was confirmed. The query is polled with a backoff (250 ms growing to 2 s)
until it is `Complete`; `Failed`, `Cancelled` and `Timeout` end it as errors (the service gives no
reason for a failure: check the query, the groups and the window). The service has its own 60-minute
cap, and 30 concurrent queries per region (*LimitExceededException*: wait for one to end).

## Commands

| Command | CloudWatch Logs call | Result |
|---|---|---|
| `groups [PREFIX] [-n\|--limit N]` | DescribeLogGroups | `log_group, retention, stored_bytes, class, metric_filters, created` |
| `streams [GROUP] [-n\|--limit N]` | DescribeLogStreams, most recently active first | `log_stream, last_event, first_event, created` (default 20) |
| `tail [GROUP] [--since DURATION] [--filter PATTERN] [-n\|--limit N]` | FilterLogEvents | `timestamp, log_stream, message`: the events of the window (default last 5 minutes), **oldest first**, at most `--limit` (default 100). `PATTERN` is a CloudWatch [filter pattern](https://docs.aws.amazon.com/AmazonCloudWatch/latest/logs/FilterAndPatternSyntax.html). There is no follow mode |
| `use [GROUP[,GROUP…]]` | none (DescribeLogGroups for `prefix*`) | the selected groups; `use` alone shows them |
| `help` | none | the commands |

`GROUP` may be left out when exactly one group is selected. Listings are bounded by `max_rows` and
`max_pages` (default 20); a listing cut short says so.

### Options (URL query)

| Option | Default | Meaning |
|---|---:|---|
| `since` | `1h` | the window of a query without one |
| `max_window` | `7d` | the widest window of a query, and of `tail --since` |
| `max_rows` | 1000 (max 10 000) | result limit of an Insights query; rows of the listings |
| `max_pages` | 20 | requests per listing / `tail` |
| `scan_limit` | 1000 | log groups `\dt` and completion list |
| `max_scan_gb` | none | stop a query that scanned more |
| `query_timeout` | the configured query timeout (30 s) | seconds before a query is stopped; `0` = the service's own 60 minutes |

## Browsing with backslash commands

| Command | Shows |
|---|---|
| `\l`, `\dt` | the account's log groups (`DescribeLogGroups`, bounded by `scan_limit`) |
| `\c GROUP` | select that group (`\c /aws/lambda/*` selects a prefix) |
| `\d GROUP` | retention, stored bytes, class, metric filter count, KMS key, data protection, ARN, and the five most recently active streams |

Nothing here starts an Insights query (that would bill): `\d` lists the standard fields only.
Tab completion offers the commands, the `since` / `between` / `--group` prefix, the Insights
commands, `@fields` and log group names.

## Read-only by construction

No command and no query word writes. The one server-side effect of a query is the billable work of
`StartQuery`, bounded as above. `--read-only` accepts every command and query and refuses anything
else; the AI assistant and the driver share that one judge (what the guard accepts is exactly what
runs). That is a statement about the **client**: what the credentials may do is decided by **IAM**,
which a client cannot prove. For a hard limit give the key a policy that only allows the actions the
commands need, on the groups it should read:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    { "Effect": "Allow",
      "Action": ["logs:StartQuery", "logs:GetQueryResults", "logs:StopQuery", "logs:FilterLogEvents",
                 "logs:DescribeLogStreams"],
      "Resource": "arn:aws:logs:eu-west-1:123456789012:log-group:/aws/lambda/orders*:*" },
    { "Effect": "Allow", "Action": ["logs:DescribeLogGroups"], "Resource": "*" }
  ]
}
```

:::note[Logs hold secrets]
Application logs routinely contain personal data and credentials. Read-only is not a confidentiality
control: use CloudWatch **data protection policies** (masking) on the groups someone should query.
:::

Connecting runs a `DescribeLogGroups`; being **denied** it does not stop the connection (a policy
that only grants `StartQuery` on one group still works), bad or expired credentials or an unreachable
endpoint do, and say why: the service error code, a hint, the request id.

The [credential broker](/user-guide/agent-connections/) **refuses** CloudWatch Logs for now
(`unsupported_engine`): IAM cannot be proven from the client, and an SDK client would inherit the broker
process' `AWS_*` environment and `~/.aws`.

## Sessions and secrets

Saved sessions and recent connections keep the log group or prefix, endpoint, region, profile, the
window options and the access key id; the **secret access key and the session token go to the
[secret store](/user-guide/secret-storage/)**, bound to the endpoint, the log group, the access key id,
the transport and the region. A key saved for `eu-west-1` is never offered to `us-east-1`, to another
group or endpoint, to a plain-HTTP endpoint, or to S3 or DynamoDB. Errors, `Debug` output and logs mask
keys, tokens and anything shaped like an access key id.

## AI assistant

`??` can query CloudWatch Logs: it is told the Insights syntax, the window prefix and the cost model,
may only run what `--read-only` accepts, and sees the same bounded results.

## Desktop app

The desktop app is built without the AWS connectors and does not offer CloudWatch Logs in its connection
form. Use the CLI.

## Testing

There is no local emulator that runs Logs Insights (LocalStack's community edition does not), so this
connector is tested against an in-process server that speaks the Logs JSON protocol: the real SDK, the
polling state machine, `StopQuery` on cancel, timeout and budget, pagination and errors. A manual smoke
test against AWS is part of the release checklist.

## What is not supported (yet)

- Writing in any form (PutLogEvents, group and stream management, retention, subscription filters,
  exports, data protection policies, saved queries).
- SQL and OpenSearch PPL query languages (`queryLanguage`), `StartLiveTail` and `tail` follow mode,
  cross-account observability (log group ARNs), `GetLogRecord` (the hidden `@ptr`).
- CloudWatch **Metrics** (a separate service and SDK).
- Agent enrollment through the broker.

## Troubleshooting

| Message | Cause |
|---|---|
| *Amazon CloudWatch Logs support is not compiled into this build* | rebuild with `--features aws` (Rust ≥ 1.94.1) |
| *no AWS region: use cloudwatch://eu-west-1, add ?region=…* | AWS needs a region; set it in the URL, `?region=`, `AWS_REGION` or a profile |
| *no log group selected* | connect to `cloudwatch://REGION/GROUP`, or `use GROUP`, or put `--group GROUP` before the query |
| *a window of … is wider than max_window* | narrow the window, or raise `max_window` |
| *MalformedQueryException* | a syntax error in the Insights query; the message says where |
| *ResourceNotFoundException* | no such log group in this region and account: `groups` lists them |
| *LimitExceededException* | 30 Insights queries are already running in the region |
| *request timed out after 30s … the query was stopped* | the configured query timeout; raise `query_timeout_seconds` or `?query_timeout=` |
| *could not confirm that query … was stopped* | StopQuery failed: the query may scan until the service's 60 minute limit |
| *stopped: the query had already scanned …* | the `max_scan_gb` budget; narrow the window or the groups |
| *AccessDeniedException* | the policy lacks `logs:StartQuery` / `GetQueryResults` / `StopQuery` / `FilterLogEvents` / `DescribeLogStreams` on the group |

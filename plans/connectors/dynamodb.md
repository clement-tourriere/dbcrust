# DynamoDB connector (`dynamodb` feature)

Wave 3.3. Read [aws-common.md](aws-common.md) first (versions, ring-only config, credentials, broker caveats).

## 1. Library choice

- **`aws-sdk-dynamodb =1.130.0`** (2026-10-01, 14 d; Apache-2.0; `aws-sdk-rust-ci`), `default-features = false, features = ["rt-tokio", "behavior-version-latest"]`, with the shared `aws-config`/ring HTTP client.
- Marginal cost on top of the shared AWS base: **+2 new crate pairs** (`aws-sdk-dynamodb`, `tokio-util` bump); standalone probe: **31** new pairs total, clean build 34 s wall / 136 s CPU, stripped 4.03 MB (**+3.66 MB** over baseline, of which aws-config base is +3.44 MB).
- No `aws-lc-*`, no OpenSSL (probe `cargo tree -i aws-lc-rs` is empty). RustSec: nothing beyond the shared rustls baseline.
- Alternatives: `dynomite`/`rusoto_dynamodb` (unmaintained), `serde_dynamo` (helper for AttributeValue <-> serde; **not needed**, we render AttributeValue to strings directly).

## 2. URL and `ConnectionInfo` mapping

- Schemes: `dynamodb://` (alias `ddb://`). `dynamodb://eu-west-1/Orders?profile=prod-ro`; local: `dynamodb://localhost:8000?region=local` (plain HTTP, dummy credentials accepted by `amazon/dynamodb-local`).
- `ConnectionInfo`: `host` = custom endpoint host (DynamoDB Local, LocalStack, VPC endpoint) or None, `port`, `username` = access key id / profile, `password` = secret access key, `database` = **table name** (optional: None lists tables), `use_tls` (false only for loopback), `options`: `region`, `profile`, `role_arn`, `endpoint`, `session_token` (secret), `consistent_read=true|false`, `index`.
- `DatabaseTypeExt`: `default_port = None` (443/8000 depending on endpoint), ssh tunnel false, `requires_authentication = true` (ambient allowed), `supports_json_explain = false`, `is_file_based = false`.
- **Secrets** as in aws-common. `TargetBinding`: engine=`dynamodb`, host=region/endpoint, port, database=table (or None = account-level), user, tls_mode.

## 3. Query model

DynamoDB is schemaless key-value/document storage with two query surfaces, both supported:

| Surface | Calls | Used for |
|---|---|---|
| **PartiQL** (default in the REPL) | `ExecuteStatement` (`SELECT * FROM "Orders" WHERE pk = 'a'`), paginated with `NextToken`, optional `Limit`, `ReturnConsumedCapacity=TOTAL` | ad-hoc SQL-like reads; the closest thing to a SQL front-end. A `SELECT` without a key condition is a **full table scan** (billed) |
| **Native typed ops** | `ListTables`, `DescribeTable`, `GetItem`, `Query` (key condition + optional index), `Scan` (`Limit`, `Segment`, `FilterExpression`), `BatchGetItem` | `\dt`, `\d`, GUI browse, agent `list_tables`/`describe_table`; deterministic cost knobs (`Limit`, `ConsistentRead`, `ProjectionExpression`) |

Result mapping to `StructuredQueryResult`:
- Columns: table key schema first (HASH, RANGE from `DescribeTable`), then the union of attribute names in the fetched page (first-seen order, capped at 200 columns, extras folded into a trailing `_other` JSON cell).
- Cells: `S`/`N`/`BOOL`/`NULL` -> text (NULL -> SQL NULL), `B` -> base64, `SS`/`NS`/`BS`/`L`/`M` -> compact JSON. Missing attribute -> NULL (distinct from the DynamoDB `NULL` type, shown as `null`).
- `LastEvaluatedKey`/`NextToken` surfaced as a notice (`more results: \next`); consumed capacity as a notice.
- Metadata: `get_schemas` -> `default` (or the region), `get_tables` -> `ListTables` (paged), `get_columns(table)` -> key attributes + `AttributeDefinitions` + a cached 1-page sample scan (`Limit 25`) *only on user request/first agent `describe_table`* (it costs RCUs), `get_table_details` -> key schema, GSIs/LSIs (as "indexes"), item count/size (stale up to 6 h), billing mode, TTL attribute, stream status. `supports_explain = false`.
- Dialect/completion: PartiQL keywords/functions (`begins_with`, `contains`, `attribute_type`, `size`, `MISSING`) + table/index names, key attribute names for `WHERE`.

## 4. Safety / broker capabilities

| Capability | Status |
|---|---|
| Read-only | **IAM only; not client-provable** (dbflux also leaves `WritePrivilege::Unknown`). `ReadOnlyKind::ClientStructural` for typed ops (only `Get/Query/Scan/BatchGet/Describe/List` exist); for PartiQL a statement classifier admits `SELECT` only. `native_read_only=false` unless `iam:SimulatePrincipalPolicy` proves `dynamodb:PutItem/UpdateItem/DeleteItem/PartiQLInsert/PartiQLUpdate/PartiQLDelete` are denied. Owner recipe (documented): allow `dynamodb:GetItem, Query, Scan, BatchGetItem, DescribeTable, ListTables, PartiQLSelect` on the table ARN; add condition `dynamodb:FullTableScan = false` on `PartiQLSelect` to forbid scans server-side |
| Classification | PartiQL: single statement, leading `SELECT` only (no `INSERT`/`UPDATE`/`DELETE`/`EXISTS`... transactional forms `ExecuteTransaction`, `BatchExecuteStatement` never exposed); native ops are a closed set. Refuse `Scan` without `Limit` in agent mode; require key condition or explicit `--allow-scan` (owner policy bit `allow_full_scan`) |
| Statement timeout | SDK `TimeoutConfig` operation/attempt - native client-side; no server statement timeout |
| Cancellation | drop the future aborts the HTTP request (native); the server may finish the page |
| Row/byte limits | **Row cap is local**: DynamoDB `Limit` counts *evaluated* items, and PartiQL SELECT reads up to a full 1 MB page before we cut (dbflux documents the same). Therefore bound cost with `Limit`, `ProjectionExpression`, `max_pages` (default 1 for agents), and report `ConsumedCapacity` (RCUs); `max_bytes` enforced on the decoded page |
| Denied | `PutItem`, `UpdateItem`, `DeleteItem`, `BatchWriteItem`, `TransactWrite*`, `ExecuteStatement` non-SELECT, `CreateTable/DeleteTable/UpdateTable`, `ImportTable/ExportTableToPointInTime` (S3 egress), backup/restore, `UpdateTimeToLive`, stream readers `GetRecords`/`GetShardIterator` unless typed and bounded |

DML opt-in (D2): technically possible for PartiQL single-item writes with a full-primary-key `WHERE`, but there is no safe "no WHERE = whole table" analogue (`DELETE` needs a key, so mass writes are impossible by design, which helps). Defer to a later phase; v1 read-only.

## 5. Testing strategy

- Inline unit tests: AttributeValue -> string/JSON table (every variant, nested maps/lists, number precision as strings, binary), column union/ordering/cap, PartiQL classifier corpus (rstest: comments, quoted identifiers containing `;`/keywords, `SELECT` inside `WHERE`, `INSERT ... VALUE`), key-condition requirement check, limit/page arithmetic, URL parsing/redaction canaries. SDK calls via `StaticReplayClient`.
- Opt-in integration (`DYNAMODB_ENDPOINT` env, skipped when unset): `docker run -p 8000:8000 amazon/dynamodb-local` (seed with the SDK in the test) and `localstack/localstack` (DynamoDB in free tier). PartiQL support in DynamoDB Local is partial - tests that need real PartiQL semantics (`FullTableScan` condition) are manual/AWS-only.

## 6. Effort, risks, feature gating

- **Effort: M** (4-5 days on top of the shared AWS base; typed ops + PartiQL + attribute rendering + capacity notices).
- Cargo feature `dynamodb` (implies `aws`). **Opt-in** (D3).
- Risks: (1) cost visibility - scans are billed per item/page, so defaults must be conservative; (2) schemaless columns make `\d` approximate and sampling costs RCUs; (3) PartiQL has no `EXPLAIN`/plan and gives no way to tell scan from query before running (only the IAM condition key stops scans); (4) read-only unprovable client-side; (5) MSRV 1.94.1 shared with all AWS crates; (6) DynamoDB Local deviates from the service for PartiQL - rely on fakes and record/replay fixtures captured once from AWS.

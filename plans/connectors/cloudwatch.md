# CloudWatch Logs connector (`cloudwatch` feature)

Wave 3.3. Read [aws-common.md](aws-common.md) first (versions, ring-only config, credentials, broker caveats). Scope: **CloudWatch Logs** (Logs Insights + direct event reads). CloudWatch *Metrics* (`aws-sdk-cloudwatch`, `GetMetricData`) is a possible later addition (dbflux ships it; the Logs-only slice is what the roadmap asked for).

## 1. Library choice

- **`aws-sdk-cloudwatchlogs =1.156.0`** (2026-10-01, 14 d; Apache-2.0; `aws-sdk-rust-ci`), `default-features = false, features = ["rt-tokio", "behavior-version-latest"]`.
- Marginal cost on top of the shared AWS base: **+3 new pairs** (`aws-sdk-cloudwatchlogs`, `aws-smithy-eventstream`, `tokio-util` bump). Standalone probe: **32** new pairs, clean build 48 s wall / 156 s CPU, stripped 3.97 MB (**+3.60 MB**). All three AWS services together: 4.37 MB.
- `aws-smithy-eventstream` is only needed for `StartLiveTail`, which we will not expose (can be left to cargo; no feature switch exists).
- No aws-lc, no OpenSSL; RustSec clean beyond the shared baseline.

## 2. URL and `ConnectionInfo` mapping

- Schemes: `cloudwatch://` (alias `cwlogs://`). `cloudwatch://eu-west-1/aws/lambda/orders-api?profile=prod-ro&window=1h&language=cwli`; several groups: `?groups=/aws/lambda/a,/aws/lambda/b`; prefix selection: `?group_prefix=/aws/lambda/`.
- `ConnectionInfo`: `host` = custom endpoint (LocalStack) or None, `port`, `username` = access key id / profile, `password` = secret access key, `database` = **log group** (first), `use_tls`, `options`: `region`, `profile`, `role_arn`, `endpoint`, `session_token` (secret), `window` (default time window, e.g. `1h`), `max_window`, `language=cwli|ppl|sql`, `groups`.
- `DatabaseTypeExt`: `default_port = None`, ssh tunnel false, `requires_authentication = true` (ambient), `supports_json_explain = false`, `is_file_based = false`.
- **Secrets** as in aws-common. `TargetBinding`: engine=`cloudwatch`, host=region/endpoint, port, database=log group (or prefix), user, tls_mode. The bound log-group set is part of the enrollment: an agent cannot query other groups.

## 3. Query model

| Mode | API | Notes |
|---|---|---|
| **Logs Insights** (default) | `StartQuery` {`logGroupNames`/`logGroupIdentifiers`, `startTime`/`endTime` (epoch seconds, **mandatory**), `queryString`, `limit` <= 10000, `queryLanguage` = CWLI / PPL / SQL} -> poll `GetQueryResults` until `status` is `Complete` (also `Scheduled`, `Running`, `Failed`, `Cancelled`, `Timeout`) -> `StopQuery` on timeout/cancel | result is `Vec<Vec<{field,value}>>`; `statistics` = `recordsMatched`, `recordsScanned`, `bytesScanned` (shown as a notice and logged to audit) |
| Direct reads | `FilterLogEvents` / `GetLogEvents` (single group/stream; no Insights charge but 1 MB / 10k event pages) | `\tail`-like commands and "describe log stream" |
| Discovery | `DescribeLogGroups` (prefix, paged), `DescribeLogStreams`, `GetLogGroupFields` | metadata |

Mapping: columns = union of returned field names in first-seen order, `@ptr` hidden (kept internally for `GetLogRecord`), `@timestamp` first; cells = strings (NULL when the field is absent in a row). SQL/PPL modes return the same shape.

DBCrust-side time window: the REPL gets `\window 15m|1h|24h|<start>..<end>` (client state, default from `options.window`); every Insights query is sent with that window - there is no query without one. For `@timestamp` relative expressions the user can also write `--since 2h` in one-shot mode. Agent mode: `timeout_secs`, `max_window` (default 24 h), `max_rows` (<= 10000, the API cap per `GetQueryResults` page), `max_bytes_scanned` (abort with `StopQuery` when `statistics.bytesScanned` exceeds it while polling).

Metadata: `get_schemas` -> `default`; `get_tables` -> log groups (`DescribeLogGroups`, prefix-filtered, capped); `get_columns(group)` -> `GetLogGroupFields` (discovered fields with percent-of-events, includes `@timestamp`, `@message`, `@logStream`, `@ingestionTime`) ; `get_table_details` -> retention, stored bytes, KMS key, data-protection flag; `get_functions` -> Logs Insights functions/commands (`fields`, `filter`, `stats`, `parse`, `sort`, `limit`, `dedup`, `pattern`, `diff`; `count`, `sum`, `avg`, `bin`, `strlen`...) for completion; `supports_explain = false`.

## 4. Safety / broker capabilities

| Capability | Status |
|---|---|
| Read-only | **No mutating call exists in the connector** (`ReadOnlyKind::ClientStructural`); IAM still decides (not client-provable) -> `native_read_only = false` unless `SimulatePrincipalPolicy` shows no `logs:Put*/Delete*/Create*`. Insights queries themselves are reads (but `StartQuery` creates billable, quota-limited server work). Owner recipe: `logs:StartQuery`, `logs:GetQueryResults`, `logs:StopQuery`, `logs:DescribeLogGroups`, `logs:DescribeLogStreams`, `logs:GetLogGroupFields`, `logs:FilterLogEvents`, `logs:GetLogEvents` scoped to the group ARNs |
| Classification | The query text is a read language; the guard is *structural + budget*, not textual. Optional agent denylist inside CWLI: none needed (no write verbs); SQL/PPL modes likewise. Refuse to run without `logGroupNames` or beyond the bound set |
| Statement timeout | polling deadline; on expiry call `StopQuery` - **native server-side cancel** (better than dbflux, which has none); Insights also has a 60 min server cap |
| Cancellation | `StopQuery` (+ dropping the poll loop); required on every error/interrupt path to stop billing |
| Row/byte limits | `limit` request field (server-side, <= 10000) for rows; byte cost bounded by window + group count (max 50 groups per query by service quota) + `max_bytes_scanned` abort; per-query result size cap ~ 10k rows/page |
| Denied | `PutLogEvents`, `DeleteLogGroup/Stream`, `PutRetentionPolicy`, `CreateExportTask` (exports to S3 = exfil + write), `PutQueryDefinition`, `PutDataProtectionPolicy`, `PutSubscriptionFilter`/`PutDestination` (stream out to third parties), `AssociateKmsKey`, `StartLiveTail` (unbounded stream; may be exposed later with caps) |

Data-sensitivity note: application logs routinely contain PII/secrets; read-only is not a confidentiality control. Recommend CloudWatch data-protection policies (masking) on the groups an agent can query, and keep audit records fingerprint-only (G.4).

DML opt-in (D2): not applicable (logs are append-only via PutLogEvents; never exposed).

## 5. Testing strategy

- Inline unit tests: result flattening (ragged rows, `@ptr`, duplicate field names, huge cells), window parsing (`15m`, `1h30m`, ISO ranges, DST-free epoch math, `max_window` clamp), poll state machine with a fake clock (Scheduled -> Running -> Complete / Failed / Timeout / deadline + `StopQuery` invariant), `max_bytes_scanned` abort, group-set binding check, URL parse + redaction canaries, pagination caps. SDK calls via `StaticReplayClient`.
- Opt-in integration (`AWS_ENDPOINT_URL`/`CWL_TEST_GROUP`, skipped when unset): LocalStack community supports the Logs *data plane* (create group/stream, `PutLogEvents`, `FilterLogEvents`) but **Logs Insights `StartQuery` is limited/unsupported in the free tier** (dbflux's CI only runs discovery/event browsing for the same reason) - so the poll state machine is covered by fakes, and a manual real-AWS smoke test is part of the release checklist.

## 6. Effort, risks, feature gating

- **Effort: M** (4-5 days on the shared AWS base; poll/cancel state machine + window UX + metadata).
- Cargo feature `cloudwatch` (implies `aws`). **Opt-in** (D3).
- Risks: (1) cost - Logs Insights bills per GB scanned; windows/groups/`bytesScanned` caps are mandatory in agent mode; (2) service quotas (30 concurrent Insights queries/region) -> per-alias concurrency 1-2; (3) no free emulator for Insights, so real-AWS manual testing; (4) CWLI/PPL/SQL language selection (`queryLanguage`) availability varies by region/log-group class; (5) result truncation at 10k rows - surface `truncated` and advise narrowing; (6) MSRV 1.94.1.

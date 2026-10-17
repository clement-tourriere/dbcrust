# InfluxDB v1 / v2 (and v3) connector (`influxdb` feature)

Wave 3.3 (rides along with the AWS wave; it has no AWS dependency).

## 1. Library choice

| Candidate | Version (age) | Notes | Verdict |
|---|---|---|---|
| `influxdb` | 0.8.0 (2026-02-19) | MIT; `Empty2k12`, `msrd0`; wraps **reqwest 0.13**; InfluxQL read/write (v1 + v2 compat) - **no Flux**, no v3 SQL. Default feature `rustls` = `reqwest/rustls` = aws-lc-rs. Probe with `rustls` feature: 3 new crates (`influxdb`, `lazy-regex`, `lazy-regex-proc_macros`), binary ~2.46 MB (aws-lc). With `default-features = false, features = ["serde"]` + our own `reqwest = { version = "0.13", default-features = false, features = ["rustls-no-provider"] }` + installed ring provider: 3 new crates, no aws-lc, ~1.93 MB (16 s wall / 88 s CPU) | Works, but buys little: no Flux/v3, ties us to reqwest 0.13 |
| `influxdb2` | 0.5.2 (2024-07-22, ~15 months stale) | MIT; Flux + v2 management API; **reqwest 0.11** (a third reqwest copy), default `native-tls` | Rejected |
| **Hand-rolled client on the locked `reqwest 0.12.28` + `csv 1.3.1`** | n/a | JSON for InfluxQL (v1) and v3 `jsonl`, annotated-CSV for Flux; both crates already DBCrust dependencies/locked; ring TLS | **Recommended: 0 new crates.** dbflux does the same (`reqwest` + `csv`, no influx crate) |

RustSec: none for the above beyond the shared `rustls 0.23.45` baseline.

## 2. Measured weight

| Variant | New pairs | Clean build (wall / CPU) | Stripped | Delta vs base |
|---|---|---|---|---|
| hand-rolled (reqwest 0.12 + csv, locked) | **0** | code only | est. +60-100 KB | est. +0.1 MB |
| `influxdb` crate, no-provider reqwest 0.13 + ring | 3 | 16 s wall / 88 s CPU | 1.93 MB | +1.56 MB |
| `influxdb` crate default (`rustls` = aws-lc) | 3 | ~68 s (loaded machine) | 2.46 MB | +2.1 MB |

## 3. URL and `ConnectionInfo` mapping

- Schemes: `influxdb://` (plain HTTP by default for localhost), `influxdbs://` (HTTPS), `influx://`/`influxdb+https://` aliases. Default port **8086** (v1/v2), **8181** (v3 Core).
- `influxdbs://host:8086/<db-or-bucket>?version=1|2|3&org=<org>&rp=<retention-policy>&dialect=influxql|flux|sql`
- `ConnectionInfo`: `host`, `port`, `username` (v1 user; v2 None), `password` (v1 password **or** v2/v3 API token - one secret slot, so the store/binding plumbing needs no new field), `database` (v1 db / v2 bucket / v3 database), `use_tls`, `options` (`version`, `org`, `rp`, `dialect`). Version is auto-detected when omitted: `GET /ping` (v1: `X-Influxdb-Version: 1.x`), `GET /health` (v2: JSON `version`), `GET /api/v3/...` probe.
- `DatabaseTypeExt`: `default_port = 8086`, ssh tunnel true, `requires_authentication = true`, `is_file_based = false`, `supports_json_explain = false`.
- **Secrets**: v1 password, v2/v3 token (`Authorization: Token <t>` / `Bearer`). Never `?u=&p=` query params (they land in server access logs): always headers. Redaction keys: `token`, `p`, `password` (already), add `influx_token`.
- `TargetBinding`: engine=`influxdb`, host, port, database=bucket/db, user=v1 user or `org`, tls_mode (+ `version`).

## 4. Query model

Three dialects, one tab-style result:

| Version | Endpoint | Language | Response | Mapping |
|---|---|---|---|---|
| v1 | `GET /query?db=&q=&epoch=ms&chunked=true&chunk_size=N` | InfluxQL | JSON `{"results":[{"statement_id","series":[{"name","tags","columns","values"}]}]}` (streamed with `chunked`) | one table: `series`/`measurement`, tag columns, then `columns`; multiple statements -> first non-empty shape + notice |
| v2 | `POST /api/v2/query?org=` (`Content-Type: application/vnd.flux`, header `Accept: application/csv`) | Flux | annotated CSV: `#datatype`, `#group`, `#default` rows + header + data, tables separated by blank lines | parse annotations -> typed strings (long, double, boolean, string, `dateTime:RFC3339`, duration); union columns across tables or one result per table with a notice |
| v2 InfluxQL | `GET /query` v1-compat (needs a DBRP mapping) with token auth | InfluxQL | v1 JSON | same as v1 |
| v3 (Core/Enterprise/Cloud) | `POST /api/v3/query_sql` / `query_influxql` `{"db","q","format":"jsonl"}` | SQL / InfluxQL | JSON lines | direct (verify field names against v3 docs in the first PR; not covered by dbflux) |

Metadata (`\dt`, `\d`, completion): schemas = databases (v1 `SHOW DATABASES`) / buckets (v2 `buckets()` or `/api/v2/buckets`) / v3 `information_schema.schemata`; tables = measurements (`SHOW MEASUREMENTS`, Flux `schema.measurements(bucket:)`); columns = `time` + tag keys (`SHOW TAG KEYS FROM m`, `schema.measurementTagKeys`) + field keys with types (`SHOW FIELD KEYS FROM m`, `schema.measurementFieldKeys`); functions = InfluxQL/Flux function names for completion (static lists via `DatabaseTypeExt::sql_functions`). `get_search_qualifiers` can expose tag keys as qualifiers. EXPLAIN: v3 `EXPLAIN` SQL; v1/v2 unsupported.

Time-series guard: unbounded `SELECT *`/`from(bucket)` scans are the main foot-gun. Agent mode requires a time bound (`WHERE time >`/`|> range(start:)`; detect structurally, not by regex over string literals as dbflux does) or injects a default window (configurable, default 1 h) plus `LIMIT`/`|> limit(n:)`.

## 5. Safety / broker capabilities

| Capability | Status |
|---|---|
| Read-only | **Mixed.** v1 over **GET**: the server itself restricts GET to `SELECT`/`SHOW` (writes, admin and `SELECT ... INTO` require POST, per the InfluxDB 1.x HTTP API docs; confirm against the targeted 1.x release in the spike) -> transport-level native for InfluxQL. v2 Flux and v3: scope is the token (read-only API token on named buckets / read-only database token) - not provable from the client without admin API rights -> `native_read_only = true` only for v1-GET, otherwise `false` (owner-attested only) and Flux relies on classification |
| Classification | InfluxQL: single statement, first keyword `SELECT`/`SHOW` only (anything else, `INTO`, `KILL QUERY`, `DROP`, `DELETE`, `CREATE/ALTER`, `GRANT` refused). Flux: allow-list imports (`array`, `date`, `strings`, `math`, `regexp`, `universe`, `influxdata/influxdb/schema`, `influxdata/influxdb`), refuse `to(`, `experimental.to`, `influxdata/influxdb/tasks`, `.../monitor`, `.../secrets` (`secrets.get` exfiltrates stored secrets), `http`/`experimental/http` (`http.post`, SSRF), `sql` (`sql.from` with DB credentials), `socket`, `kafka`, `mqtt`, notification packages, `experimental/...` writers |
| Statement timeout | client HTTP timeout + server `query-timeout`/`--query-timeout` (v1 `[coordinator] query-timeout`, v2 `--query-timeout`); not settable per request |
| Cancellation | by closing the HTTP connection (v1 `chunked` and v2 CSV streams are abandoned; server stops on disconnect - verify per version). `KILL QUERY` is v1 admin only |
| Row/byte limits | true fetch-side: stream v1 `chunked` JSON / v2 CSV / v3 JSONL and stop at the row/byte budget, then drop the connection. Plus injected `LIMIT`/`limit()` |

DML opt-in (D2): not offered in v1. Writes would be line-protocol `POST /write|/api/v2/write`, which is not a query; keep the connector read-only (matches dbflux).

## 6. Testing strategy

- Inline: a tiny `hyper`/raw-TCP fake serving canned v1 JSON, v2 annotated CSV (multi-table, empty, error body, `#default` values, quoted newlines), v3 JSONL; version detection from `/ping`/`/health`; time-guard and Flux import allow-list corpus (rstest, including string literals/comments containing `to(`); token redaction canaries in error text.
- Opt-in integration (`INFLUX_URL` + `INFLUX_TOKEN`, skipped when unset): `influxdb:1.11` (`INFLUXDB_DB=test`), `influxdb:2.7` (`DOCKER_INFLUXDB_INIT_MODE=setup`, `..._ADMIN_TOKEN`), `influxdb:3-core` (`--without-auth` or a created token). Seed with line protocol over `/write`.

## 7. Effort, risks, feature gating

- **Effort: M** (3-5 days; two wire formats + annotated-CSV parser + time guard + metadata).
- Cargo feature `influxdb`; no new crates; **default: yes** (D3: not heavy). Roadmap-wise it ships in the AWS wave but is independent.
- Risks: (1) Flux is being de-emphasised by InfluxData (v3 dropped Flux) - ship InfluxQL + Flux (v1/v2) first, SQL (v3) behind runtime version detection; (2) v3 API details need verification against current docs/server; (3) read-only cannot be proven from the client for v2/v3 tokens -> broker must treat InfluxDB as heuristic-read-only unless v1-GET; (4) annotated-CSV edge cases (multi-table, schema changes, error tables with `error`/`reference` columns); (5) unbounded time-series scans -> the time guard is mandatory for agents.

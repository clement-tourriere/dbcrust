# Amazon Redshift connector (`redshift` feature)

Wave 3.2. Redshift is a PostgreSQL-8.0.2-derived warehouse that speaks the PG wire protocol (v3). This is an *adaptation* of the existing PostgreSQL client, not a new driver stack.

## 1. Library choice

| | |
|---|---|
| Crate | **none new**: reuse `sqlx 0.8.6` (`postgres`, `runtime-tokio-rustls`), already locked and already the PG driver (`src/database_postgresql.rs`, 3010 lines) |
| License / provenance | n/a (existing SQLx/launchbadge) |
| TLS | SQLx `runtime-tokio-rustls` -> rustls, the same provider story as PG today (`ring`). Redshift endpoints use Amazon-issued certs; `sslmode=require` (default for this scheme) or `verify-full` with the Amazon root bundle (webpki roots contain Amazon Root CA 1-4) |
| RustSec | nothing new |
| Name trap | the crates.io crate named `redshift` is an unrelated 2016 colour-temperature library - never depend on it |
| Rejected | dbflux uses sync `postgres` + `native-tls` (**OpenSSL**) - not acceptable; `tokio-postgres` would add a second PG client stack (not in the lock) for no gain |
| Later (AWS wave) | `aws-sdk-redshiftdata` (Redshift Data API: `ExecuteStatement`/`DescribeStatement`/`GetStatementResult`) for IAM-only access without network reachability, and `aws-sdk-redshift` `GetClusterCredentials`/`redshift-serverless` `GetCredentials` for IAM-token passwords. Same shared AWS config as the S3/DynamoDB/CloudWatch connectors |

## 2. Measured weight

- New crates vs lock: **0**. Binary size delta: code only (catalog SQL strings + a flavour enum), estimated < 30 KB; no build-time impact. (No scratch build needed: nothing new is compiled.)

## 3. URL and `ConnectionInfo` mapping

- Schemes: `redshift://`, alias `redshift+postgres://`. Default port **5439**. `sslmode` default `require` (not `prefer`).
- `redshift://user:pass@cluster.abc123.eu-west-1.redshift.amazonaws.com:5439/dev?sslmode=verify-full&sslrootcert=/path/amazon.pem&options=...`. Serverless: `workgroup.<acct>.<region>.redshift-serverless.amazonaws.com`.
- `ConnectionInfo`: `host`, `port`, `username`, `password` (secret), `database`, `use_tls` (always true), `options` (`sslmode`, `sslrootcert`, `application_name`, later `iam_profile`, `cluster_identifier`, `region`, `db_user`).
- `DatabaseTypeExt`: ssh tunnel true (bastion in private VPC is the common path), `requires_authentication = true`, `supports_json_explain = false`.
- **Secrets**: `password`; later IAM `access_key_id`/`secret_access_key`/`session_token` (through the shared AWS auth, never in options). Add `iam_session_token` to the secret-ish keys.
- `TargetBinding`: engine=`redshift`, host, port, database, user, tls_mode.
- Implementation: add `DatabaseType::Redshift`; `PostgreSQLClient::new_with_flavor(Flavor::Redshift)`; the factory arm in `create_database_client` maps `Redshift` to the same client with the flavour set. `url_scheme.rs`: add `UrlScheme::Redshift`; `config.rs::legacy_database_type` -> `Some(Legacy::PostgreSQL)`-like entry (or a new `Legacy::Redshift` with `port 5439`).

## 4. Query model

SQL, single dialect (Redshift SQL = PG 8-ish + extensions). Everything the PG client does keeps working; differences to encode behind the flavour:

| Area | Redshift behaviour | Adaptation |
|---|---|---|
| Startup / type introspection | no `pg_range`, partial `pg_proc`/`pg_type`; SQLx looks up unknown type OIDs in `pg_catalog` | decode via text/`handle_custom_postgresql_type` fallback (CLAUDE.md rule 6 already requires `.or_else(...)`); never run pgvector/`pg_extension` probes; prefer `sqlx::raw_sql` (simple protocol) for user statements if a spike shows extended-protocol quirks |
| Catalog | `pg_table_def` only lists search_path schemas | use `SVV_TABLES`/`SVV_COLUMNS` (and `SVV_ALL_*`/`SVV_REDSHIFT_*` on Serverless/datashares), `SVV_TABLE_INFO` for `\d+` (size, `diststyle`, `sortkey1`, `skew`), `PG_NAMESPACE` for schemas, `PG_GET_VIEWDEF`/`SVV_VIEW...` where available |
| Types | `SUPER`, `VARBYTE`, `GEOMETRY`, `GEOGRAPHY`, `HLLSKETCH`, `TIMETZ`; no arrays | render unknown as text; `SUPER` as JSON text |
| Indexes / triggers / FKs | not enforced / absent; PK/FK are informational | show distkey/sortkey in `\d` instead of indexes |
| EXPLAIN | text plan only | `supports_json_explain = false`, `explain_query` -> `EXPLAIN <sql>` text |
| Session | `statement_timeout` (ms) supported; `SET search_path` ok | set `statement_timeout` at connect from `query_timeout()` |
| Cancel | `pg_cancel_backend(pid)` / `CANCEL <pid>` | reuse the PG client's interrupt flag path (`database_postgresql.rs:968`) |
| COPY/UNLOAD | `COPY ... FROM s3://`, `UNLOAD` write to S3 | classified as write/admin, denied for agents |

Metadata trait: `get_schemas` (non-system schemas), `get_tables` (tables + views + late-binding views), `get_columns`, `get_table_details` (columns + distkey/sortkey + `encode` + PK/FK informational), `get_functions` (`PG_PROC` subset), `default_schema` = `public`.

## 5. Safety / broker capabilities

| Capability | Status |
|---|---|
| Read-only | **Transaction-scoped native**: the AWS `BEGIN` reference lists `BEGIN [ISOLATION LEVEL ...] [ READ WRITE | READ ONLY ]`, so every agent statement runs as `BEGIN READ ONLY; <stmt>; ROLLBACK`. A *session-level* default (`default_transaction_read_only`) is not documented for Redshift - treat as unsupported. This must be confirmed against a real cluster in the spike (the dbflux README, which also targets Redshift, rejects writes only at its own wire layer and keeps `WritePrivilege::Unknown`). Defence in depth: enrollment wizard probes `HAS_TABLE_PRIVILEGE`/`HAS_SCHEMA_PRIVILEGE`/`HAS_DATABASE_PRIVILEGE` and `SVV_USER_INFO` (`usesuper`, `usecreatedb`); `native_read_only = true` only if the transaction mode is honoured *and* the user has no write/DDL privilege (least-privilege recipe: `CREATE USER agent PASSWORD ...; GRANT USAGE ON SCHEMA ...; GRANT SELECT ON ...`) |
| Classification | reuse `safety::check_sql` + extend: refuse `COPY`, `UNLOAD`, `CREATE EXTERNAL ...`, `CALL` (stored procedures), `VACUUM`, `ANALYZE`, `GRANT/REVOKE`, `SET SESSION AUTHORIZATION`, `CANCEL`/`ABORT`, `EXECUTE`, `PREPARE` |
| Statement timeout | server-side `statement_timeout` (native) + WLM queue timeouts |
| Cancellation | server-side via `CANCEL`/`pg_cancel_backend` from a second connection (needs same user) - native |
| Row/byte limits | fetch-side: SQLx `fetch()` stream, stop at the budget and drop the connection (cursor-based `DECLARE ... CURSOR` for very large results is an option because Redshift supports cursors within a transaction, with a leader-node materialisation caveat) |

## 6. Testing strategy

- Inline: flavour-branch SQL builders (rstest string snapshots for `SVV_*` metadata queries), URL parsing/redaction (`redshift://u:p@host:5439/dev?sslmode=require`), classifier extensions, type fallback rendering, `BEGIN READ ONLY` wrapper (multi-statement refusal).
- Local smoke without Redshift: the shared PG paths run against `postgres:16` (existing `DATABASE_URL` harness); Redshift-specific catalog SQL cannot run there.
- Opt-in live suite `REDSHIFT_URL` (skipped when unset): a real provisioned cluster or a Redshift Serverless workgroup (small base RPU; billed). No free emulator exists (LocalStack Redshift is Pro). Mark every Redshift-only query as an `#[ignore]`d live test, as dbflux does.

## 7. Effort, risks, feature gating

- **Effort: S-M** (about 3 days + 1 day spike against a real endpoint).
- Cargo feature `redshift` (depends on the existing `postgres` driver; adds no crates). **Default build: yes.**
- Risks: (1) SQLx extended-protocol/type-introspection incompatibilities are only knowable on a real cluster -> do the spike *first*, with `raw_sql` as fallback; (2) read-only `BEGIN READ ONLY` semantics unverified; (3) IAM auth is a separate (AWS-wave) deliverable, so v1 = user/password through the secret store; (4) no CI coverage without a paid cluster; (5) `pg_table_def` search_path trap in metadata; (6) Data-API alternative changes the result model (async statement ids) - keep it out of v1.

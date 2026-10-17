# SQL Server / Azure SQL connector (`mssql` feature)

Wave 3.1. Measured 2026-10-15 against the `Cargo.lock` snapshot (see README "Method").

## 1. Library choice

| | |
|---|---|
| Crate | `tiberius` **=0.13.0** (2026-09-25, ~20 days old; 0.12.3 is 2024-07-19) |
| License | MIT OR Apache-2.0 |
| Owners | Prisma team (`aqrln`, `pimeys`, `tomhoule`, `Weakky`, `miguelff`) + original author `steffengy`; repo `prisma/tiberius`, actively maintained (commits through 2026-09-25: new ADO connection-string parser, bulk-insert collation fixes) |
| MSRV | 1.88 |
| Runtime | runtime-agnostic (`futures` AsyncRead/Write); with tokio use `tokio-util` `compat` (already locked, 0.7.16) |
| Alternatives | `odbc-api` (needs unixODBC + MS driver, C), `mssql-client`/`sqlx-mssql` (SQLx removed MSSQL in 0.7; third-party forks unvetted) -> tiberius is the only pure-Rust TDS client |

### TLS story (the finding that matters)

| Option | Result |
|---|---|
| tiberius 0.13.0 `native-tls` (default feature) | OpenSSL on Linux. **Rejected** |
| tiberius 0.13.0 `rustls` | works, but the crate **hard-codes `rustls::crypto::aws_lc_rs`** as the fallback provider (`src/client/tls_stream/rustls_tls_stream.rs:24,285`) and declares `tokio-rustls = "0.26.4"` with *default features* (which include `aws_lc_rs`). Probe tree contains `aws-lc-rs 1.18.1` + `aws-lc-sys 0.45.0` (cmake/C build). At runtime a process-installed provider *is* honoured (`CryptoProvider::get_default()`), so ring works functionally, but aws-lc is still compiled and linked. **Cannot be turned off from the outside** |
| tiberius 0.12.3 `rustls` | ring-based but on **rustls 0.21.12 / rustls-webpki 0.101.7 / rustls-pemfile 1.0.4**: `cargo audit` reports RUSTSEC-2026-0104, -0098, -0099 (+ unmaintained 2025-0134). Violates the "no new advisories" policy. **Rejected** |
| tiberius 0.13.0 + 4-line patch (below) | **ring only, no aws-lc, audit clean.** Measured |

Patch (`[patch.crates-io] tiberius = { git = "<fork>", rev = "..." }`, or vendored `third_party/tiberius`; also send upstream as a `rustls-ring` feature):

```diff
--- Cargo.toml
 [dependencies.tokio-rustls]
 version = "0.26.4"
 optional = true
+default-features = false
+features = ["ring", "tls12", "logging"]
--- src/client/tls_stream/rustls_tls_stream.rs
-        crypto::{aws_lc_rs, CryptoProvider},
+        crypto::{ring as aws_lc_rs, CryptoProvider},   // used only in the no-default-provider fallback + 2 tests
```

With the patch the probe builds with 0 `aws-lc-*` crates. Recommendation: carry the patch in a fork pinned by rev for the first release (cost: one rebase per tiberius release) and open the upstream PR; fall back to "aws-lc is already compiled by reqwest 0.13 in DBCrust's tree today" only if Wave 2a has *not* removed it, and re-check when it does. `check_cargo_cooldown.py` must be run against the fork's lock entries (git source, so the 24 h rule is on the rev's commit date - note in the PR).

Features: `rustls`, `tds73` (datetime2/date/time; `tds80` is the default for newer SQL Server and implies `tds73`), `sql-browser-tokio` (named instances via SQL Browser UDP 1434), `chrono` (locked), `rust_decimal` (locked 1.43.0). Skip `integrated-auth-gssapi` (system libgssapi), `sspi-rs`, `winauth` (windows-only default) until Windows-integrated auth is requested.

## 2. Measured weight

| Variant | New crate pairs vs lock | Clean release build | Stripped binary | Delta vs base |
|---|---|---|---|---|
| tiberius 0.13.0 `rustls` (upstream, aws-lc forced) | 7: `tiberius`, `tiberius-macros`, `asynchronous-codec`, `connection-string`, `futures-timer`, `pretty-hex`, `secrecy` (aws-lc-* already in lock via rustls default feature; **re-adds a cmake C build if the lock is cleaned**) | 27 s wall / 102 s CPU | 2.38 MB | +2.01 MB |
| **tiberius 0.13.0 patched (ring only)** | 7 (same) | 14 s wall / 53 s CPU | 1.62 MB | **+1.24 MB** |
| tiberius 0.12.3 `rustls` (old rustls 0.21) | 10 (incl. second rustls/webpki/tokio-rustls/pemfile/sct) | measured 35 s (loaded machine) | 1.41 MB | +1.0 MB; **3 advisories** |

## 3. URL and `ConnectionInfo` mapping

- Schemes: `mssql://`, alias `sqlserver://`. `sqlserver://[user[:password]@]host[:port|\instance][/database]?encrypt=required|on|off&trust_server_certificate=false&application_name=dbcrust&instance=SQLEXPRESS&intent=readonly`.
- Default port **1433**; `instance` option (or `host\instance`, which needs `sql-browser-tokio`) resolves the port over UDP 1434.
- `ConnectionInfo`: `host`, `port`, `username`, `password` (secret), `database`, `use_tls` (= `encrypt` != off), `options` (`instance`, `trust_server_certificate`, `application_name`, `auth=sql|aad_token|windows`).
- `DatabaseTypeExt`: ssh tunnel true, `requires_authentication = true`, `is_file_based = false`, `supports_json_explain = false` (SHOWPLAN XML only).
- **Secrets**: `password`; Azure AD access token (`aad_token`, short-lived, never persisted); client-cert key passphrase. `trust_server_certificate=true` is a downgrade of TLS verification: record it in the `TargetBinding` tls mode and refuse it for broker-enrolled aliases.
- `TargetBinding`: engine=`mssql`, host, port (or instance name), database, user, tls_mode.

## 4. Query model

SQL (T-SQL). `Client::simple_query` for ad-hoc text; `Client::query` with `@P1` parameters for metadata. Result mapping: stream `Row` -> `ColumnData` -> `Option<String>` (NULL preserved via `execute_query_structured`); implement display for `Numeric` (rust_decimal), `DateTime2/DateTimeOffset/Date/Time` (chrono), `Guid`, `Binary` (hex, truncated), `Xml`, `Money`. Multiple result sets (`QueryStream::into_results`): show the first, list the rest as notices (same as the PG/MySQL clients).

Metadata for `\dt` / `\d` / completion (INFORMATION_SCHEMA + catalog views):
- schemas: `sys.schemas` (exclude `sys`, `INFORMATION_SCHEMA`, fixed roles unless asked)
- tables/views: `sys.tables` / `sys.views` joined to `sys.schemas`
- columns: `INFORMATION_SCHEMA.COLUMNS` + `sys.columns` (`is_identity`, computed)
- indexes: `sys.indexes` + `sys.index_columns`; FKs: `sys.foreign_keys` + `sys.foreign_key_columns`; checks: `sys.check_constraints`
- functions: `sys.objects` where `type IN ('FN','IF','TF','P')`
- databases: `sys.databases`; `connect_to_database` -> `USE [db]` is *not* safe with a pooled connection - reconnect with the new `database` in `Config`.
- `explain_query`: `SET SHOWPLAN_XML ON` (batch-scoped) -> returns the plan XML as text (TUI parser later).
- `get_server_info`: `SERVERPROPERTY('ProductVersion'|'Edition'|'EngineEdition')`.
- Dialect: reuse the existing T-SQL-agnostic `sql_parser` in "generic" mode, bracket-quoting `[ident]`, `TOP n` for auto-limit (not `LIMIT`) -> `sql_parser_trait` needs a `quote_char = '['..']'` + `limit_clause` entry.

## 5. Safety / broker capabilities

| Capability | Status |
|---|---|
| Read-only | **Not natively enforceable from the client.** `ApplicationIntent=ReadOnly` only routes to a readable AG secondary and is explicitly *not* enforcement (a write still succeeds if the connection lands on the primary). T-SQL has no `SET TRANSACTION READ ONLY`. Native options are server-side: a login mapped to `db_datareader` + `DENY INSERT, UPDATE, DELETE, EXECUTE, ALTER`, or a database set `READ_ONLY` / a read-only replica. Declare `native_read_only = false` unless the enrollment wizard verifies it: connect, then probe `HAS_PERMS_BY_NAME(db_name(), 'DATABASE', 'INSERT')`, `IS_MEMBER('db_owner')`, `IS_SRVROLEMEMBER('sysadmin')`, `DATABASEPROPERTYEX(db_name(),'Updateability')` and accept `native_read_only = true` only when the principal has no write/execute/alter and no sysadmin/db_owner |
| Classification | Single statement only, classified with a conservative tokenizer (no new parser dep): allow a leading `SELECT` or a `WITH` CTE that ends in `SELECT`, plus the internal `SET SHOWPLAN_*`/`SET STATISTICS` used for explain. Refuse: `EXEC`/`EXECUTE`, `sp_*`, `xp_*` (`xp_cmdshell`), `OPENROWSET`, `OPENQUERY`, `OPENDATASOURCE`, `BULK INSERT`, `SELECT ... INTO`, `INSERT/UPDATE/DELETE/MERGE`, `DBCC`, `BACKUP/RESTORE`, `WAITFOR`, `GRANT/DENY/REVOKE`, `ALTER/CREATE/DROP/TRUNCATE`, `KILL`, `RECONFIGURE`, `USE` (database is part of the binding), linked-server four-part names, `OUTPUT` clauses |
| Statement timeout | no server option in TDS client: `tokio::time::timeout` + drop connection; optionally `SET LOCK_TIMEOUT n` (lock waits only) |
| Cancellation | tiberius does **not** expose the TDS Attention packet. Dropping the connection kills the session (server cancels). One connection per agent request, or discard on timeout |
| Row/byte limits | stream rows (`QueryStream`) and stop at the budget, then drop the connection (no more rows pulled; server stops on disconnect). Server-side cap: `SET ROWCOUNT n` before the statement (honoured for SELECT; deprecated for DML only) and/or inject `TOP (n)` at the outermost SELECT when classification can prove it is safe |

DML opt-in (D2) is plausible here (real tables): allow `INSERT/UPDATE/DELETE` single statements (no `MERGE`, no `OUTPUT`, no triggers-aware guarantees), always refuse DDL/`EXEC`.

## 6. Testing strategy

- Inline unit tests: connection-string/URL parsing with canaries (`sqlserver://sa:p%40ss@host\SQLEXPRESS/db`), ColumnData -> string formatting (rstest table of every type incl. NULL/empty), classifier corpus (comments `--`, `/* */`, nested, string literals containing `;`/keywords, `GO` batch separator rejected, CTE-with-write), TOP injection, metadata SQL builders (quoting `]` -> `]]`).
- Fake: tiberius needs a TDS server; no good in-process fake -> test the mapper on `ColumnData` values directly and keep wire behaviour to the integration suite.
- Opt-in integration (`MSSQL_URL` env, skipped when unset): `docker run -e ACCEPT_EULA=Y -e MSSQL_SA_PASSWORD=... -p 1433:1433 mcr.microsoft.com/mssql/server:2022-latest` (arm64: `mcr.microsoft.com/azure-sql-edge` is retired; use Rosetta/`--platform linux/amd64`, or `mssql/server:2022` on Apple silicon with Rosetta enabled). Self-signed cert -> `trust_server_certificate=true` in test only. A second run creates a `db_datareader` login to assert the read-only probe returns true, and an `sa` run asserts false.

## 7. Effort, risks, feature gating

- **Effort: M-L** (6-8 days): driver + type formatting + T-SQL metadata + completion/quoting dialect + read-only probe + fork/patch maintenance.
- Cargo feature `mssql`. Default build: **yes, but only once ring-only is achieved** (patched tiberius). With upstream tiberius it must be opt-in (aws-lc C build, +0.8 MB).
- Risks: (1) tiberius rustls fork/patch carrying cost; upstream may accept a `rustls-ring` feature; (2) no TDS cancel -> no true cancellation; (3) read-only is a *server-side* property, so the broker must refuse MSSQL enrollment as "native read-only" unless the probe passes; (4) SQL Server on Apple-silicon docker is slow/needs Rosetta; (5) Windows integrated auth (Kerberos/NTLM) deferred; (6) Azure AD token acquisition belongs to the later AWS/Azure auth wave (user supplies a token for now); (7) legacy collations/code pages - 0.13 added opt-in lossy decoding, enable only on request.

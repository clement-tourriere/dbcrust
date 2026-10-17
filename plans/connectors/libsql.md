# libSQL / Turso (remote) connector (`libsql` feature)

Wave 3.2. Scope: **remote only** (Turso Cloud and self-hosted `sqld`/libsql-server over HTTP, Hrana). Local files stay on the existing SQLx SQLite driver; embedded replicas/sync are out of scope (no second SQLite C build).

## 1. Library choice

Three candidates were built and audited (probes, DBCrust lock + DBCrust release profile):

| Candidate | Version (age) | License / owners | New pairs | Clean build (wall / CPU) | Stripped size | Verdict |
|---|---|---|---|---|---|---|
| `libsql` `remote`+`tls` | **0.10.0** (2026-10-13, ~44 h) | MIT; `penberg`, `glommer`, team `github:tursodatabase:publish` | **20** (old stacks: `hyper 0.14`, `http 0.2`, `h2 0.3.27`, `rustls 0.22.4`, `rustls-webpki 0.102.8`, `tokio-rustls 0.25`, `tower 0.4`, `prost 0.12`) | 20 s wall / 74 s CPU | 2.39 MB (++2.02 MB) | **Rejected**: `cargo audit` = 5 vulnerabilities (RUSTSEC-2026-0258 `h2 0.3.27`; RUSTSEC-2026-0104/-0098/-0099/-0049 `rustls-webpki 0.102.8`) + `rustls-pemfile` unmaintained; policy forbids suppressing new advisories. Also a duplicate legacy HTTP/TLS stack. Ring-based (no aws-lc), but not usable |
| `turso_serverless` | **0.1.3** (2026-09-04, ~41 d) | MIT; `penberg` only | **1** | 30 s wall / 133 s CPU | 2.96 MB (++2.59 MB) | Audit clean (baseline rustls only). **But** depends on `reqwest 0.13` with *default features* -> `default-tls` -> rustls with `aws-lc-rs` (aws-lc-sys C build) and `rustls-platform-verifier`; cannot be turned off from the dependency edge. Acceptable only while reqwest 0.13/aws-lc is in the tree anyway (genai today); it would re-block the Wave 2a ring unification. dbflux uses exactly this crate (`=0.1.3`) |
| **Hand-rolled Hrana client on the locked `reqwest 0.12.28`** | n/a | n/a | **0** | code only | ~+40-60 KB est. | **Recommended.** reqwest 0.12 is already a direct DBCrust dependency (`json`, `rustls-tls`, `stream`, ring). ~500 lines of Rust: JSON in/out, no protobuf, and full control of `describe`/cursor/timeouts |

Hrana-over-HTTP (JSON encoding), as documented in the libsql repo `docs/HTTP_V2_SPEC.md` (v2) and its v3 successor: `POST {base}/v2/pipeline` with `{"baton": null, "requests": [{"type":"execute","stmt":{"sql":"...","args":[...],"want_rows":true}}, {"type":"close"}]}`, `Authorization: Bearer <token>`; response `{"baton","base_url","results":[{"type":"ok","response":{"type":"execute","result":{"cols":[{"name","decltype"}],"rows":[[{"type":"integer","value":"1"}]],"affected_row_count","last_insert_rowid","rows_read","rows_written","query_duration_ms"}}}]}`; values are tagged (`null`, `integer` as decimal string, `float`, `text`, `blob` base64). Request types of interest: `execute`, `batch`, `sequence`, `describe` (returns `params`, `cols`, `is_explain`, **`is_readonly`**), `close`, `get_autocommit`. Verify exact field names against the spec in the first PR (this document only paraphrases it); pin server compatibility with `GET /v2` (and `/v3`) probes.

If the owner later prefers the crate, the minimal acceptable form is `turso_serverless` after Wave 2a (single reqwest 0.13, `rustls-no-provider` + ring) - but that requires upstream to expose a non-default-tls feature.

## 2. Measured weight

See the table above (probes built with the DBCrust release profile; `turso_serverless` adds 1 crate but forces reqwest 0.13's aws-lc path, +~+2.59 MB in the binary). The recommended hand-rolled client adds no dependencies.

## 3. URL and `ConnectionInfo` mapping

- Schemes: `libsql://` (HTTPS by default), `turso://` (alias), `libsql+http://` / `http://host:8080` (plain, local sqld only; refused for non-loopback hosts unless `?tls=false` is explicit).
- `libsql://my-db-myorg.turso.io[?namespace=ns]`; token **never** in the URL for agent/enrolled use (the legacy `?authToken=` form is accepted for humans and must be added to the redaction key list: `authtoken`, `auth_token`).
- `ConnectionInfo`: `host`, `port` (default 443 when TLS, 8080 for plain), `username` None, `password` = auth token (JWT) so the secret-store plumbing, binding and `password_sanitizer` handle it unchanged, `database` = namespace (sqld multi-tenant `x-namespace` header / Turso single db -> None), `use_tls`, `options` (`encryption_key` for Turso remote-encrypted DBs).
- `DatabaseTypeExt`: `default_port = 443`, ssh tunnel false (HTTP/TLS to a public edge), `is_file_based = false`, `requires_authentication = true`, `supports_json_explain = false`; dialect = SQLite (`sql_parser_sqlite`, `"` quoting).
- **Secrets**: auth token, remote-encryption key (base64). Both non-persistable outside the store.
- `TargetBinding`: engine=`libsql`, host, port, database=namespace, user=None (token id from the JWT `id`/`a` claim may be recorded as non-secret descriptor), tls_mode.
- `DatabaseType::LibSql` (+ `UrlScheme::LibSql`), keep `DatabaseType::SQLite` untouched.

## 4. Query model

SQL (SQLite dialect) over Hrana. `execute_query_structured`: one `execute` request -> `cols` + tagged `rows` -> `Option<String>` (NULL preserved; blobs as hex/`<N bytes>`). Multi-statement scripts use `sequence`/`batch`. Interactive transactions (baton-carried stream) only for humans; agent mode is stateless (`baton: null`, `close` in the same pipeline) - this also makes `PRAGMA query_only` unusable as a session guard (see below).

Metadata: reuse the SQLite driver's SQL (`sqlite_schema`, `pragma_table_info(...)`, `pragma_index_list`, `pragma_foreign_key_list`) executed through Hrana; `get_schemas` -> `main` (+ attached none); `explain_query` -> `EXPLAIN QUERY PLAN`; `get_server_info` -> `select sqlite_version()` + (Turso) `GET /version`. Query stats (`rows_read`, `rows_written`, `query_duration_ms`) surface as notices and are valuable for agent cost control (Turso bills rows read/written).

## 5. Safety / broker capabilities

| Capability | Status |
|---|---|
| Read-only | **Native via token**: Turso read-only tokens (`turso db tokens create --read-only`) and sqld JWTs with access claim `ro`; server rejects writes (`SQLITE_READONLY`). The client cannot prove token scope by probing (a probe would have to write). Decode the token's non-secret claims when it is a JWT (`a == "ro"`) and declare `native_read_only = true` only then; otherwise `false` (owner attestation is not accepted as native). Additional per-statement server check: Hrana `describe` returns `is_readonly` (SQLite `sqlite3_stmt_readonly`) -> refuse any statement where it is false. This is the SQLite parser, not a heuristic; the spike must still list statements for which it is true but that have side effects (`ATTACH`, `PRAGMA` writes, `load_extension`) and keep those in the textual denylist |
| Classification | `safety::check_sql` + SQLite PRAGMA rules (already present) + `describe.is_readonly` |
| Statement timeout | client-side `tokio::time::timeout`; server enforces its own limits; no per-request server knob in Hrana |
| Cancellation | none in the protocol; dropping the HTTP request is best effort (dbflux README: "no query cancellation") |
| Row/byte limits | v2 pipeline returns the whole result (fetch-side truncation after transfer; row cap only bounds retained rows). v3 `/v3/cursor` streams NDJSON entries and allows true early-stop - use it when the server advertises v3. Inject `LIMIT` for plain SELECTs via the existing auto-limit logic |

Agent write opt-in (D2) works at token level only: owner issues a read-write token; DBCrust still refuses DDL/PRAGMA writes/ATTACH/`load_extension`/`VACUUM INTO`.

## 6. Testing strategy

- Inline unit tests with a **fake Hrana server** (`tokio` TCP + hand-written HTTP/1.1 or `hyper` is already locked): canned `pipeline` responses for execute/describe/error/baton, value-tag decoding (integer string, float, blob base64, null), multi-statement, 401/403/429/5xx mapping, token redaction canaries in errors, `is_readonly` refusal.
- Opt-in integration (`LIBSQL_URL` + `LIBSQL_TOKEN`): `docker run -p 8080:8080 ghcr.io/tursodatabase/libsql-server:latest` (no auth for the basic run; JWT run with `SQLD_AUTH_JWT_KEY` to test `ro`/`rw` tokens), plus an optional real Turso free-tier DB.

## 7. Effort, risks, feature gating

- **Effort: M** (4-5 days: client + value mapping + metadata + describe-based classifier + GUI form + tests).
- Cargo feature `libsql`; no new crates; **default build: yes** (D3: not heavy).
- Risks: (1) protocol drift (Hrana v2/v3 field names; Turso is moving products - `turso`/`turso_serverless` crate renames; pin tests to a fake + a docker `sqld`); (2) read-only guarantee depends on token scope, not client-provable -> be conservative in `native_read_only`; (3) no cancel; (4) hand-rolling means we own bug fixes (batons expiring, `base_url` redirect for sticky routing, gzip) - mitigated by a small, test-covered surface; (5) crate alternative `turso_serverless` may become viable after Wave 2a - keep the client behind a trait so it can be swapped.

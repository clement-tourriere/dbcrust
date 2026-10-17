# Redis connector (`redis` feature)

Wave 3.1. Measured 2026-10-15 against the `Cargo.lock` snapshot of branch `ctourriere/secure-agent-connectors` (see README "Method").

## 1. Library choice

| | |
|---|---|
| Crate | `redis` **=1.7.1** (published 2026-09-25, ~20 days old, not yanked) |
| License | BSD-3-Clause |
| Owners | `mitsuhiko`, team `github:redis-rs:release-team` (redis-rs org, 115M downloads, long-lived project) |
| MSRV | 1.88 (DBCrust: 1.91) |
| RustSec | no advisory on `redis` or its new deps. `cargo audit` on the probe shows only the shared `rustls 0.23.45` advisories (RUSTSEC-2026-0346..0349, fixed in 0.23.46, released 2026-10-14) that DBCrust's lock already has |
| Runtime | tokio (`tokio-comp`) |
| TLS | `tokio-rustls-comp` + `tls-rustls-webpki-roots`. `redis` depends on `rustls` and `tokio-rustls` with `default-features = false`, so it forces **neither aws-lc-rs nor OpenSSL** |
| Alternatives rejected | dbflux uses `redis 0.27` (old); `deadpool-redis`/`bb8` (pool layer, not needed: `MultiplexedConnection`/`ConnectionManager` already multiplex); `fred` (larger, not evaluated) |

Features to use:

```toml
redis = { version = "=1.7.1", default-features = false, features = [
    "tokio-rustls-comp",        # tokio + rustls
    "tls-rustls-webpki-roots",  # bundled roots; add `tls-rustls` (native roots) only if OS roots are wanted
    "connection-manager",       # auto-reconnect wrapper
    "streams",                  # XRANGE/XINFO typed helpers (optional)
] }
# later, same crate: "cluster-async", "sentinel"
```

Do **not** enable `default` (adds `script` -> `sha1_smol`, `num-bigint`, `geospatial`, `acl` helper types we do not need), `tokio-native-tls-comp`/`tls-native-tls` (OpenSSL), `smol-*`, `entra-id` (azure SDK).

**Provider caveat (important):** `redis` calls `rustls::ClientConfig::builder()`, which needs a process default `CryptoProvider`. While DBCrust's tree still contains both `ring` and `aws-lc-rs` (reqwest 0.13 via genai) `builder()` is ambiguous and panics. DBCrust must install the ring provider once at startup (`rustls::crypto::ring::default_provider().install_default()`); the probe does exactly this. This is the same helper SQL Server and AWS need, so put it in one `tls` module in `dbcrust-core`.

**Cooldown note:** a fresh resolve picked `combine 4.6.9` (published 2026-10-15 13:17 UTC, 1.2 h old). `redis` needs `combine ^4.6.8`; pin with `cargo update -p combine --precise 4.6.8` (2026-08-25) and run `check_cargo_cooldown.py`.

## 2. Measured weight (probe: tokio + redis, DBCrust release profile `opt-level=z`, thin LTO, strip)

| Variant | New crate pairs vs lock | Clean release build | Stripped binary | Delta vs tokio-only probe |
|---|---|---|---|---|
| base (tokio only) | 0 | 7 s wall / 9 s CPU | 0.37 MB | - |
| `redis` (features above) | **5**: `redis`, `arcstr`, `backon`, `xxhash-rust`, `combine` (4.6.7 -> >=4.6.8) | 16 s wall / 51 s CPU | 1.63 MB | **+1.26 MB** |
| + `cluster-async`, `sentinel` | 6 | 19 s wall / 56 s CPU | 1.63 MB | +1.26 MB |

The cluster probe does not call cluster APIs, so LTO strips them and the size equals the plain probe; real cluster use adds code. No C/C++ build, no build scripts of note. Everything else (`rustls`, `tokio-rustls`, `webpki-roots`, `socket2`, `url`, `bytes`, `tokio-util`) is already locked.

## 3. URL and `ConnectionInfo` mapping

- Schemes: `redis://`, `rediss://` (TLS), alias `valkey://` / `valkeys://` (wire-compatible). Optional later: `redis+unix:///path`.
- `redis://[user[:password]@]host[:port][/db]`
- `ConnectionInfo`: `host`, `port` (default **6379**), `username` = ACL user (None -> `default`), `password` (secret), `database` = logical DB index as text (`0`..`15`), `use_tls` = `rediss`, `options`: `mode=standalone|cluster|sentinel`, `sentinel_master=NAME`, `client_name`, `connect_timeout`, `response_timeout`.
- `DatabaseTypeExt`: `supports_ssh_tunnel = true`, `is_file_based = false`, `requires_authentication = false` (optional), `supports_json_explain = false`.
- **Secrets** (secret store, never TOML/URL in agent mode): `password`; sentinel password (`sentinel_password`); TLS client key path is non-secret but the key passphrase (if any) is secret. Add `sentinel_password` to the secret-ish key list in `password_sanitizer`.
- `TargetBinding`: engine=`redis`, host, port, database=db index, user, tls_mode. `tls_insecure` must not be accepted for broker-enrolled aliases (the redis feature `tls-rustls-insecure` is deliberately **not** enabled).

## 4. Query model

Not SQL. Input is a redis-cli style command line, tokenized with shell-like quoting (`SET "a b" 'c d'`). Results map to the tabular `StructuredQueryResult`:

| RESP reply | Rows |
|---|---|
| simple string / integer / bulk | one row, column `value` |
| nil | one row, `value = NULL` |
| array of scalars | rows `idx`, `value` (or just `value`) |
| flat array that is a known pair shape (`HGETALL`, `ZRANGE ... WITHSCORES`, `CONFIG GET`, `XRANGE` entries) | `field`,`value` / `member`,`score` / `id`,`field`,`value` |
| `SCAN`/`SSCAN`/`HSCAN` | iterate the cursor client-side until the row/byte budget, report `truncated` |
| nested array / map / set / push | JSON text in a `value` column |

Typed operations (used by `\dt`, GUI browser, agent `list_tables`/`describe_table`) so users do not need `KEYS`:

- **List "tables"**: `SCAN 0 MATCH <pattern> COUNT 500` capped (default 10k keys sampled), grouped by key prefix up to the first `:`; each key's `TYPE`, `TTL` via a pipeline.
- **Describe key**: `TYPE`, `TTL`, `OBJECT ENCODING`, `MEMORY USAGE`, cardinality (`STRLEN`/`LLEN`/`HLEN`/`SCARD`/`ZCARD`/`XLEN`), plus a bounded sample (`HSCAN`, `SSCAN`, `LRANGE 0 N`, `ZRANGE 0 N`, `XRANGE - + COUNT N`).
- `MetadataProvider`: `get_schemas` -> non-empty DB indexes from `INFO keyspace`; `get_tables(schema)` -> sampled prefixes; `get_columns(key)` -> hash field names (sampled) or `value`; `get_functions` -> command names from `COMMAND LIST` for completion (arity/flags from `COMMAND INFO`); `supports_explain` -> false; `default_schema` -> `db<N>`.
- `list_databases` -> `db0..` from `INFO keyspace`; `connect_to_database("N")` -> `SELECT N` (denied for agents: the DB is part of the binding).
- Completion: command names + (for the first argument) key names from the sampled cache.
- `get_server_info`: `INFO server` -> `redis_version`, `redis_mode`, `os`.

## 5. Safety / broker capabilities

| Capability | Status |
|---|---|
| Read-only | **Native when the ACL proves it, else heuristic.** Redis >= 7: after connect run `ACL WHOAMI` then `ACL DRYRUN <user> SET k v`, `... DEL k`, `... FLUSHALL`, `... CONFIG SET maxmemory 0`, `... EVAL "return 1" 0`; if every canary is denied, declare `native_read_only = true`. Redis < 6 (no ACLs) and managed services that block `ACL` -> `Heuristic` (client classification only). A replica (`replica-read-only yes`, default) rejects writes natively but still executes admin commands, so it is not sufficient alone |
| Classification | Use the server's own command table: `COMMAND INFO <cmd>` flags (`write`, `admin`, `dangerous`, `blocking`, `pubsub`, `noscript`) + an explicit allow list of read commands. Unknown or module commands are refused. Subcommands matter (`CONFIG GET` vs `CONFIG SET`, `CLIENT LIST` vs `CLIENT KILL`, `OBJECT ENCODING`, `XINFO`) |
| Statement timeout | per-connection `response_timeout` (`AsyncConnectionConfig::set_response_timeout`) + `tokio::time::timeout`; no per-command server-side timeout |
| Cancellation | **None** for a running command on a multiplexed connection. Recovery = drop the connection (agent mode: one connection per request or discard on timeout). `CLIENT KILL ID` needs a second connection and the `CLIENT` permission |
| Row/byte limits | **Fetch-side only**: the crate reads a whole reply before returning. So agent mode must bound the *request*: rewrite unbounded forms (`LRANGE k 0 -1`, `SMEMBERS`, `HGETALL`, `ZRANGE 0 -1`, `XRANGE - +`, `GETRANGE`) to bounded ones or pre-check cardinality/length (`LLEN`, `SCARD`, `HLEN`, `ZCARD`, `STRLEN` <= cap) before running |

**Commands denied for agents** (even with a "read-only" label): `FLUSHALL`, `FLUSHDB`, `CONFIG` (all), `EVAL`, `EVALSHA`, `FUNCTION`, `SCRIPT`, `MODULE`, `DEBUG`, `SHUTDOWN`, `SAVE`, `BGSAVE`, `BGREWRITEAOF`, `REPLICAOF`/`SLAVEOF`, `FAILOVER`, `MIGRATE`, `RESTORE`, `ACL`, `AUTH`, `HELLO`, `SELECT`, `CLIENT` (except internal `SETNAME`), `MONITOR`, `KEYS` (use `SCAN`), `SUBSCRIBE`/`PSUBSCRIBE`/`PUBLISH`, every blocking command (`BLPOP`, `BRPOP`, `BLMOVE`, `XREAD ... BLOCK`, `WAIT`), `SORT ... STORE`, `GEORADIUS ... STORE`, `XREADGROUP`/`XACK`/`XCLAIM`/`XAUTOCLAIM`, `OBJECT FREQ` (needs LFU policy; harmless but noisy), `SLOWLOG RESET`, `LATENCY RESET`, `MEMORY PURGE`.

DML opt-in (D2) for Redis is *not* recommended in v1: there is no table/row concept for the ceiling to bind to; allow only an explicit key-prefix allow list (`~prefix:*` is best expressed server-side as an ACL key pattern, which the owner creates).

Recommended least-privilege ACL (documented in the user guide): `ACL SETUSER dbcrust-agent on >PASSWORD ~* resetchannels -@all +@read -@dangerous -@admin -keys -@scripting +info +scan +type +ttl +pttl +object|encoding +memory|usage`.

## 6. Testing strategy

- Pure inline unit tests (rstest): command tokenizer (quotes, escapes, empty args), RESP -> rows mapper per shape, classifier corpus (every denied command above, subcommands, case-insensitivity, `MULTI`/`EXEC`, pipelines in one line), bounded-rewrite function, URL parse/redaction canaries (`redis://:pw@h`, `rediss://u:p@h/2`, IPv6).
- Fake server: ~80-line `tokio::net::TcpListener` speaking RESP2 with canned replies (no docker) to test connect, `AUTH`, `SELECT`, ACL canary probe, timeout and drop-on-timeout.
- Opt-in integration (`REDIS_URL`, skipped when unset, like `DATABASE_URL`): `docker run -p 6379:6379 redis:7-alpine`; second run with `valkey/valkey:8`; an ACL variant (`--user agent on '>pw' '~*' '+@read'`) to assert `native_read_only = true`; a TLS variant (`--tls-port`, test CA).

## 7. Effort, risks, feature gating

- **Effort: M** (4-6 working days): client + RESP mapper + classifier + metadata + completion + GUI form + docs.
- Cargo feature `redis`. Small enough for the default build (5 crates, ~+1.26 MB): **default on** (D3 only requires opt-in for heavy connectors).
- Risks: (1) no cancellation -> agent requests need one connection per request or a drop-on-timeout policy; (2) managed Redis (ElastiCache/Upstash) often blocks `CLIENT`, `ACL`, `CONFIG`, `COMMAND` -> every probe must degrade to Heuristic, never fail connect; (3) cluster mode multiplies metadata cost (SCAN per node) - ship standalone first, `cluster-async` later (measured above); (4) RESP3 push messages need `?protocol=3` handling - stay on RESP2 initially; (5) combine 4.6.9 cooldown pin.

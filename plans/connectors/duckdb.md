# DuckDB (+ DuckLake) connector (`duckdb` feature)

Wave 3.4, last and heaviest. Today `DatabaseType::DuckDB` and the `duckdb://` scheme exist but route to the DataFusion file engine (`src/database.rs:694,860`, `src/database_datafusion.rs`); that is **not** DuckDB. This connector replaces that arm when the feature is on and keeps the DataFusion fallback (with an explicit notice) when it is off.

## 1. Library choice

| | |
|---|---|
| Crate | `duckdb` **=1.10506.0** (+ `libduckdb-sys =1.10506.0`), published 2026-09-30 (15 d), engine version **1.5.6** (the probe's download mode fetched `v1.5.6`; crate version `1.10506.0` encodes it) |
| License | MIT |
| Owners | `Mytherin` (Mark Raasveldt, DuckDB Labs) and `wangfenjin` (original duckdb-rs author); repo `duckdb/duckdb-rs` (official org) |
| MSRV | 1.85.1 |
| Runtime | **synchronous** API (`Connection` is `!Sync`). Run on a dedicated blocking thread per connection (`spawn_blocking`/the GUI per-connection worker already has one). Not a tokio crate, which is fine |
| `arrow` | depends on `arrow ^58`; DBCrust locks `arrow 58.1.0` -> **no second Arrow**. Render values with `arrow::util::display` so output matches the DataFusion engine |
| RustSec | nothing new beyond the shared `rustls 0.23.45` baseline |
| TLS | none in the crate. **DuckDB's `httpfs` (S3/HTTP) extension is C++ with its own TLS (OpenSSL/mbedtls inside the extension binary)** and is *not* part of the `bundled` build; it is a runtime-downloaded extension (see 5) |
| Build-time deps | `bundled`: `cc` (a C++17 compiler: clang/g++/MSVC). Non-bundled: build script `ureq`(rustls)+`flate2`+`tar`+`zip` to download the release (see "Non-bundled"). `bundled-cmake` needs `cmake` |

### Bundled vs non-bundled (measured, DBCrust release profile, tiny probe that opens a file DB)

| Mode | New crate pairs vs lock | Clean build wall / CPU | Stripped probe | Runtime artefacts |
|---|---|---|---|---|
| `bundled` (C++ amalgamation compiled by `cc`) | **7**: `duckdb`, `libduckdb-sys`, `cast`, `fallible-iterator`, `fallible-streaming-iterator`, + second versions `crossterm 0.28.1`, `rustix 0.38.44` (from arrow `prettyprint` -> `comfy-table`) | 159 s wall / 1056 s CPU (first measurement under load average ~25: 3 m 57 s wall, 968 s user + 181 s sys) | 19.48 MB | none |
| `bundled` + `parquet` + `json` | 7 | 5 m 02 s wall, 1115 s user + 244 s sys (loaded machine) | 22.5 MB | none |
| non-bundled, `DUCKDB_DOWNLOAD_LIB=1` | 7 | **30 s** wall, 110 s user (+ build-time download from GitHub releases) | **770 KB** | `libduckdb.dylib`/`.so`/`.dll` **117 MB** unstripped (full release with extensions statically included) next to the binary; Cargo does not set an rpath (`dyld: no LC_RPATH's found` when run) - needs `-C link-arg=-Wl,-rpath,...` or `DYLD_LIBRARY_PATH`/`LD_LIBRARY_PATH` |
| non-bundled, `DUCKDB_LIB_DIR` / pkg-config / vcpkg | 7 | seconds | ~770 KB | a system `libduckdb` (Homebrew `duckdb`, distro packages) must match the crate's `=1.10506.0` engine ABI exactly |

Verdict: **bundled is the shippable mode** (static, one binary, same build story as SQLx's bundled SQLite but ~15x the compile time and +20 MB). Non-bundled is a viable *developer/Docker/Nix* mode (30 s builds, no C++ toolchain) and must be supported by the same feature via the crate's env vars, but it cannot back PyPI wheels or `mise run install` on a clean machine because it needs a network fetch at build time or a pre-installed shared library of the exact version.

Wheel/CI implications (CLAUDE.md mentions 32-bit Python wheels): DuckDB does not target 32-bit; the `duckdb` feature must be **excluded from i686 wheel builds**, and from musl/cross builds until proven. mbx caching matters (~18 CPU-minutes cold).

## 2. URL and `ConnectionInfo` mapping

- Existing scheme `duckdb:`; add `ducklake:`.
- `duckdb:///abs/path.duckdb[?read_only=true|false&threads=N&memory_limit=2GB&extensions=ducklake,httpfs]`, `duckdb://./relative.db`, `duckdb://:memory:` (humans only; an in-memory DB cannot be opened read-only: measured error `Cannot launch in-memory database in read-only mode!`).
- DuckLake: `ducklake:sqlite:///path/meta.sqlite?data_path=/data/lake/` or `ducklake:postgres://user:pass@host:5432/catalogdb?data_path=s3://bucket/lake/&alias=lake`. Executed as `INSTALL ducklake; LOAD ducklake; ATTACH 'ducklake:postgres:dbname=... host=...' AS lake (DATA_PATH '...')` by the connector at open time (human mode, before configuration is locked).
- `ConnectionInfo`: `file_path` = database file (the DataFusion arm already populates `file_path`); `database` = attached catalog alias; for DuckLake `host`/`port`/`username`/`password`/`database` describe the *metadata catalog* (Postgres), `options`: `data_path`, `alias`, `extensions`, `read_only`, `memory_limit`, `threads`, `temp_directory`.
- `DatabaseTypeExt` (already present): `is_file_based = true`, `default_port = None`, ssh tunnel false for files (true for a DuckLake Postgres catalog), `requires_authentication = false` (true for DuckLake-Postgres), `supports_json_explain = true`.
- **Secrets**: DuckLake catalog password, object-store credentials for `data_path` (DuckDB persistent secrets - `CREATE SECRET` writes plaintext-ish files under `~/.duckdb/stored_secrets`: must be **disabled**, supply secrets as in-memory `CREATE TEMPORARY SECRET` from the secret store, never persisted). Add `data_path_secret`, `s3_secret_key`, `s3_session_token` to redaction keys. DuckDB file encryption key (1.4+ `ENCRYPTION_KEY`) is also a secret.
- `TargetBinding`: engine=`duckdb`/`ducklake`, host/port = catalog host (None for file), database = file path (canonicalised) or catalog alias, user = catalog user, tls_mode. **The canonical file path is the binding**: refuse a symlink/`..` swap after enrollment.

## 3. Query model

SQL (DuckDB dialect, PostgreSQL-flavoured; GUI completion already uses the PostgreSql dialect for it, `gui/core/src/completion.rs:53`). Execute through `Connection::prepare`/`query_arrow` -> `RecordBatch`es -> `Option<String>` cells via `arrow::util::display::ArrayFormatter` (NULL preserved); nested `LIST/STRUCT/MAP/UNION` render as JSON-ish text, same as the DataFusion path.

Metadata (`\dt`, `\d`, completion): `duckdb_databases()`, `duckdb_schemas()`, `duckdb_tables()`/`duckdb_views()` (+ `information_schema.*` for portability), `duckdb_columns()`, `duckdb_indexes()`, `duckdb_constraints()`, `duckdb_functions()` (for completion + `sql_functions`), `duckdb_extensions()`, `PRAGMA database_size`. System catalogs filtered the same way `gui/core/src/schema.rs:209` already does (`duckdb_*`, `sqlite_*`). `EXPLAIN` text and `EXPLAIN (FORMAT JSON)`; `EXPLAIN ANALYZE` executes the statement (same rule as `explain_analyze_query_raw`). File queries (`SELECT * FROM 'x.parquet'`, `read_csv`, `read_json`) work natively with the `parquet`+`json` features - this makes DuckDB a **superset engine** of the DataFusion files engine; keep DataFusion for the default build, route `duckdb://` to this driver only when the feature is on. `get_server_info`: `SELECT version()`.

## 4. Safety / broker capabilities (empirically verified)

Probe (`duckdb 1.10506.0`, file database, `Config`: `access_mode=READ_ONLY`, `enable_external_access=false`, `autoinstall_known_extensions=false`, `autoload_known_extensions=false`, `lock_configuration=true`), results:

```
OK   select a, b from t
DENY create table z(i int)              -> Cannot execute statement of type "CREATE" on database ... attached in read-only mode!
DENY copy t to '/tmp/x.csv'             -> Permission Error: ... file system operations are disabled by configuration
DENY install httpfs                     -> Permission Error: Cannot access directory ".../extensions/..." - file system operations are disabled
DENY set enable_external_access=true    -> Cannot change configuration option ... the configuration has been locked
DENY select * from read_csv('/etc/hosts') -> Permission Error: ... file system operations are disabled by configuration
DENY attach '/tmp/other.db'             -> Permission Error: ... file system operations are disabled by configuration
```

| Capability | Status |
|---|---|
| Read-only | **Native** for file databases: `access_mode=READ_ONLY` (engine-enforced; multiple read-only processes may open the same file). Not available for `:memory:`. `native_read_only = true` iff the file DB was opened with the flag *and* the lock was applied (verify via `SELECT current_setting('access_mode')`). For DuckLake the catalog (Postgres) credentials must be read-only at the DB level - the DuckDB flag covers the attached `ducklake` catalog only through `ATTACH ... (READ_ONLY)` (spike) |
| Hardening beyond RO | `enable_external_access=false` (blocks file reads/writes, `COPY`, `ATTACH`, `INSTALL`/`LOAD`), `lock_configuration=true` (no re-enabling), `autoinstall/autoload_known_extensions=false`, `allow_community_extensions=false`, `allow_unsigned_extensions=false`, `memory_limit`, `threads`, `max_temp_directory_size`, `enable_http_logging=false`; use `allowed_directories` as the exception list when the owner wants specific Parquet/CSV directories readable (documented DuckDB behaviour; the probe did not test it - spike item). Without external access disabled a `READ ONLY` transaction can still read arbitrary files/URLs (dbflux's README says the same) |
| Classification (defence in depth) | `SELECT json_serialize_sql(?)`: DuckDB's own parser serialises **only** SELECT statements and errors for everything else - an authoritative single-statement SELECT test (multi-statement strings are also rejected by `prepare` extracting >1 statement). Deny list regardless: `INSTALL`, `LOAD`, `ATTACH`, `DETACH`, `COPY`, `EXPORT`/`IMPORT DATABASE`, `SET`/`RESET`/`PRAGMA` (except a read allow list: `database_size`, `table_info`, `show_tables`), `CALL`, `CREATE SECRET`/`DROP SECRET`, `CHECKPOINT`, `VACUUM`, DDL/DML; table functions `read_*`, `glob`, `sniff_csv`, `parquet_*`, `duckdb_secrets()`, `getenv`, `list_files`, `http_*` |
| Statement timeout | no setting; implement with `Connection::interrupt_handle().interrupt()` from a timer (present in 1.10506.0: `src/lib.rs:671`) |
| Cancellation | **native**: same `InterruptHandle` (also wire to DBCrust's `interrupt_flag()` as the PG client does) |
| Row/byte limits | fetch-side: iterate Arrow batches, stop at budget, `interrupt()`; plus auto-`LIMIT` on plain SELECTs; resource caps via `memory_limit`/`max_temp_directory_size` |

DML opt-in (D2): possible for a DuckDB *file* (open read-write, external access still off, DDL refused by classifier); offer only for explicitly enrolled local/DuckLake targets.

## 5. DuckLake and extensions

DuckLake is a DuckDB **extension** (`ducklake`), not part of the `bundled` crate features (the crate only statically links `json`, `parquet`, `icu`, `tpch`, `tpcds`, `autocomplete`). `INSTALL ducklake` downloads a signed binary from `extensions.duckdb.org` at runtime (network + filesystem write under `~/.duckdb/extensions/<version>/<platform>/`), and `httpfs` (S3 data paths) likewise. Therefore:

- Human sessions: connector runs `INSTALL`/`LOAD` for the allow-listed extensions (`ducklake`, `httpfs`, `parquet`, `json`) at open, *then* applies hardening + `lock_configuration`.
- Agent/broker sessions: **no network installs ever**. Provision the extension directory once (`extension_directory` setting pointing at a read-only, owner-managed dir), and open with autoinstall off. A missing extension is a connect error, not a download.
- Version coupling: extension binaries are per DuckDB version/platform; bump `duckdb` and the extension cache together.
- Do not advertise DuckLake until: catalog connect (SQLite/Postgres), `data_path` object-store secrets via `CREATE TEMPORARY SECRET`, `allowed_directories` + external-access interplay, and read-only attach are tested end to end (spike). DuckLake time-travel (`AT (VERSION => n)`) works as plain SQL.

## 6. Testing strategy

- **Real tests need no Docker**: with `bundled`, unit tests open temp-file and `:memory:` databases directly. Inline rstest: hardening matrix above as a regression test (each statement -> expected DENY), `json_serialize_sql` classifier corpus (CTE, `COPY (SELECT ...) TO`, `SELECT ... INTO`-like, `PRAGMA`, multi-statement, comments/quotes), URL parsing + canonical path binding (symlink, `..`, relative), Arrow -> string formatter table (all type families, NULL vs empty), interrupt/timeout test (a `range(1e12)` cross join cancelled within N ms), metadata queries against a fixture DB.
- CI: dedicated `duckdb` job (cold bundled build ~5 min wall on a 12-core runner; use the mbx cache), excluded from i686/32-bit wheels.
- Opt-in `DUCKLAKE_TEST=1` job: downloads the extension (network), SQLite-catalog DuckLake in a temp dir; Postgres-catalog variant with the existing `DATABASE_URL`.

## 7. Effort, risks, feature gating

- **Effort: L** - 8-12 days for the engine driver, hardening, metadata, GUI form/schema browser, CI wiring; DuckLake **+M** (5 days) as a separate sub-phase; non-bundled env support is mostly docs + CI matrix.
- Cargo feature `duckdb` (implies `libduckdb-sys/bundled`; sub-features `duckdb-parquet`, `duckdb-json`). **Opt-in** (D3): +20 MB binary, ~18 CPU-minutes cold build, C++ toolchain requirement. Never in the default/slim presets.
- Risks: (1) C++ build time and toolchain requirements on every CI platform (Linux aarch64 cross, musl, Windows MSVC, manylinux) and exclusion from 32-bit wheels; (2) synchronous API + `!Sync` connection -> careful threading in the GUI/worker model; (3) extension auto-install/network behaviour must be disabled in broker mode (verified settings above); (4) file-reading table functions and persistent secrets are the exfil/write vectors if hardening is partially applied - keep the hardening all-or-nothing and unit-tested; (5) crate version scheme (`1.10506.0` = engine 1.5.6) and a fast release cadence (monthly) - pin exactly, re-measure per bump; (6) two Arrow consumers (DataFusion and DuckDB) must stay on the same major - currently both 58; (7) the existing `DatabaseType::DuckDB` -> DataFusion routing must keep working for builds without the feature (documented behavioural difference).

---
title: "DuckDB & DuckLake"
description: Open DuckDB database files and in-memory databases with the embedded engine, query Parquet/CSV/JSON in place, attach DuckLake catalogs, and run it read-only with the engine itself refusing writes, file access and extensions.
---

# DuckDB & DuckLake

DBCrust embeds the [DuckDB](https://duckdb.org) engine: no server to run, no client library to
install. Point it at a database file, or at nothing (an in-memory database), and type DuckDB SQL.

```sh
dbcrust duckdb:///data/warehouse.duckdb
dbcrust warehouse.duckdb                              # the .duckdb / .ddb extension is enough
dbcrust 'duckdb://:memory:' -c "SELECT * FROM 'events.parquet' LIMIT 10"
dbcrust duckdb:///data/warehouse.duckdb --read-only -c 'SELECT count(*) FROM orders'
```

**DuckDB is an opt-in build feature.** The engine is a C++ library compiled from source into the
binary, which costs a long cold build (a C++17 compiler is required) and about 20 MB, so it is in
neither `bundled` nor `standard`. Without it, a `duckdb://` URL fails with *"DuckDB support is not
compiled into this build (enable the `duckdb` cargo feature, or install a full build)"*:

```sh
cargo install --path . --locked --features duckdb          # the default build plus DuckDB
cargo install --path . --locked --no-default-features --features standard,duckdb
cargo run --features duckdb -- duckdb:///tmp/test.duckdb   # from a checkout
```

See [Build features](/user-guide/development/#build-features) for the measured cost, and the
[Python wheels and the 32-bit build](#builds-wheels-and-ci) note below.

Until this connector existed, `duckdb://` was routed to the Parquet/CSV/JSON engine (Apache
DataFusion), which cannot read a DuckDB database. That route is gone: a build with `duckdb` uses
the real engine, a build without it says so. Parquet, CSV and JSON files keep their own URLs
(`parquet://`, `csv://`, `json://`) in every build with the `files` feature; DuckDB reads them too,
natively.

## Connecting

```text
duckdb:///abs/path/file.duckdb[?options]     # a database file (created if it does not exist)
./relative.duckdb                            # a bare path is resolved against the working directory
duckdb://:memory:    duckdb://               # an in-memory database
path/to/file.duckdb  file.ddb                # inferred from the extension
```

| Option | Default | Effect |
|---|---|---|
| `access_mode` | `automatic` | `read_only` opens the file read-only (`read_only=true` is the same). Several processes can share a file opened read-only. An in-memory database cannot be read-only |
| `memory_limit` | DuckDB's (80 % of RAM) | e.g. `2GB`, `512MiB` |
| `threads` | all cores | a positive number |
| `max_rows` | `100000` | rows kept from one result set (`0` = unlimited); the read stops there, the rest is never fetched |
| `extensions` | none | comma-separated extensions to `LOAD` at connect (`httpfs,spatial`) |
| `install_extensions` | `false` | `true` lets DBCrust `INSTALL` those extensions, and lets DuckDB auto-install known ones on first use. **That downloads signed binaries from DuckDB's extension repository (network access)**; nothing is ever downloaded without this opt-in |
| `ducklake`, `data_path`, `alias` | — | attach a [DuckLake](#ducklake) catalog |
| `allowed_directories` | none | folders a `--read-only` connection may read, see [Read-only](#read-only) |

Opening a file that does not exist creates an empty database there, as the `duckdb` shell does;
with `--read-only` it is an error and nothing is created. A file that is not a DuckDB database
fails with DuckDB's own message.

DuckDB has no network protocol, so there are no hosts, users or passwords here. The same DuckDB
file cannot be opened read-write by two processes at once (DuckDB's file lock): close the other
session, or open it read-only.

## Running SQL

The session is one DuckDB connection for as long as DBCrust runs, so `SET`, `USE`, temporary
tables and open transactions persist between statements. Several statements in one input run in
order and the last result is shown.

- **Results** stream from the engine and stop at `max_rows`; a huge `SELECT` is never held in memory.
- **Ctrl-C and `query_timeout_seconds`** stop a running statement with DuckDB's own `interrupt`;
  the session stays usable (*"query interrupted"* / *"query timed out after 30s and was cancelled"*).
- **Commands** such as `CREATE TABLE` or `SET` answer `Status: OK`; DML answers DuckDB's `Count`.
- **Files are tables**: `SELECT * FROM 'sales/*.parquet'`, `read_csv('x.csv', header = true)`,
  `read_json('events.ndjson')`. The Parquet and JSON extensions are compiled in, so this needs
  no download.
- **`EXPLAIN`** shows DuckDB's physical plan as text (the GUI draws it in sections); `EXPLAIN
  ANALYZE` executes the statement and adds timings.

### How values are shown

| DuckDB type | Shown as |
|---|---|
| `DECIMAL`, `HUGEINT`, `BIGINT`… | every digit, exact (`12345678901234567890.123456789`) |
| `TIMESTAMP`, `DATE`, `TIME` | `2024-01-02 03:04:05.123456`, trailing zeros of the fraction trimmed, `infinity` / `-infinity` |
| `TIMESTAMP WITH TIME ZONE` | in UTC: `2024-01-02 01:04:05+00` |
| `INTERVAL` | `1 year 2 months 3 days 04:05:06.5` |
| `BLOB` | printable bytes as is, the rest as `\xAA` |
| `UUID`, `ENUM`, `VARCHAR` | their text |
| `LIST`, `ARRAY`, `STRUCT`, `MAP`, `UNION` | JSON text: `[1,2,3]`, `{"a":1,"b":[1.5,null]}`; numbers and booleans stay bare, a `DECIMAL` keeps its digits, dates and everything else is a JSON string |
| `NULL` | an actual NULL, distinct from `''` |

Two limits of the engine's Arrow export: a `UHUGEINT` above 2^127 − 1 prints as a negative
number (cast it to `VARCHAR` or `DECIMAL(38,0)` in the query), and a `HUGEINT` of 39 digits does
not fit an Arrow decimal.

## Browsing

`\dt` lists the tables and views of the current schema, `\d table` shows columns, `NOT NULL`,
defaults, the primary key, unique constraints, explicit indexes, `CHECK` and foreign keys, and the
tables that reference this one. `\l` lists the **databases attached to the session** (`memory`,
your file, anything you `ATTACH`, a DuckLake) with the current one marked; `\c name` is `USE
name`. Completion offers DuckDB's own catalog and function names. Names you type are never
spliced into the catalog queries: they travel as parameters.

## Read-only

`--read-only` (and the AI agent, and the [agent broker](/user-guide/agent-connections/)) opens the
file with the engine's own protections, not with a filter in front of it:

| Setting | Effect |
|---|---|
| `access_mode = READ_ONLY` | the database file cannot change: `INSERT`, `CREATE`, `DROP`, `CHECKPOINT`… are refused by DuckDB (*"Cannot execute statement of type … in read-only mode"*) |
| `enable_external_access = false` | no `COPY … TO`, `EXPORT DATABASE`, `ATTACH`, `INSTALL`/`LOAD`, and no `read_csv('/etc/passwd')`, `glob()`, `read_text()`, URLs or `FROM 'file.csv'`: *"file system operations are disabled by configuration"* |
| `lock_configuration = true` | nothing can switch any of that back on (`SET enable_external_access = true` is refused) |
| extensions | autoinstall and autoload off, community and unsigned extensions off |

That makes read-only **engine-enforced** for a file database, which is rare among DBCrust's
engines. In-memory databases cannot be opened read-only (DuckDB refuses), so `--read-only
duckdb://:memory:` is an error rather than a promise it cannot keep.

On top of that the usual [textual guard](/ai-agents/safety/) runs, so a refused statement gets a
clear message before it is sent: only `SELECT`, `WITH … SELECT`, `FROM … SELECT`, `VALUES`,
`SHOW`, `DESCRIBE`, `SUMMARIZE`, `PIVOT`, `EXPLAIN` and read-only `PRAGMA`s
(`table_info`, `database_size`, `show_tables`…) pass; the file and network table functions
(`read_*`, `glob`, `parquet_*`, `sqlite_scan`, `postgres_scan`, `getenv`, `duckdb_secrets`,
`query`…) and files named where a table belongs (`FROM '/data/x.parquet'`) are refused.

Reading files while read-only is deliberately off. To allow one folder, name it:

```sh
dbcrust duckdb:///data/warehouse.duckdb?allowed_directories=/data/landing --read-only \
  -c "SELECT * FROM read_csv('/data/landing/today.csv')"
```

DuckDB lets a connection **read and write** inside an allowed folder, so only allow folders you
would let it write to. The folder list is set before the configuration is locked and cannot be
changed afterwards. The broker never accepts this option.

DuckDB's temporary catalog (`CREATE TEMP TABLE`) lives in memory and is not the database file, so
it still works read-only.

### Agents and the broker

A **local DuckDB file** can be enrolled for agents, read-only:

```sh
dbcrust broker enroll warehouse 'duckdb:///data/warehouse.duckdb'
```

The enrollment binds the file's canonical path (a symlink or `..` spelling binds the same file;
at connect time the path must still resolve to it, otherwise the alias asks to be re-enrolled),
takes no options, no password and no `--allow-writes` (there is no writable DuckDB profile that
keeps `COPY`, `ATTACH` and the file functions closed, so agents never get one), and refuses
`:memory:`, DuckLake and extensions. The gateway connects with the profile above and classifies
each statement with the broker's lexer on top; see [Agent Connections](/user-guide/agent-connections/).
A broker built without the `duckdb` feature refuses the alias with the usual *"not compiled"*
message.

## DuckLake

[DuckLake](https://ducklake.select) keeps a lakehouse catalog (tables, snapshots, schema changes)
in an ordinary database — a DuckDB file, SQLite or PostgreSQL — and the data as Parquet files
under a `DATA_PATH`. It is a DuckDB **extension** (`ducklake`), not part of the compiled-in
engine, so using it takes one deliberate step: allowing DuckDB to download it.

```sh
# SQLite catalog, local data
dbcrust 'duckdb://:memory:?ducklake=sqlite:/srv/lake/meta.sqlite&data_path=/srv/lake/data/&install_extensions=true'

# PostgreSQL catalog (the password comes from PGPASSWORD or ~/.pgpass, never from the URL)
PGPASSWORD=… dbcrust 'duckdb://:memory:?ducklake=postgres:dbname=lake%20host=db.internal&data_path=s3://bucket/lake/&alias=sales&install_extensions=true&extensions=httpfs'
```

At connect DBCrust runs, once and before the session is yours:

```sql
INSTALL ducklake; LOAD ducklake;              -- INSTALL only with install_extensions=true
ATTACH 'ducklake:sqlite:/srv/lake/meta.sqlite' AS lake (DATA_PATH '/srv/lake/data/');
USE lake;
```

`\dt`, `\d`, `\l` and completion then work on the lake like on any attached database, and
DuckLake's time travel is plain SQL (`SELECT * FROM events AT (VERSION => 3)`, `FROM
lake.snapshots()`). The alias defaults to `lake`; `read_only=true` attaches it `READ_ONLY`.

- **The extension is fetched once**, from DuckDB's extension repository over HTTPS, into
  `~/.duckdb/extensions/<version>/<platform>/`. Offline, or without `install_extensions=true`,
  the connection fails with *"could not load the DuckDB extension 'ducklake' … (add
  install_extensions=true to download it, which needs network access)"*. Extension binaries are
  per DuckDB version, so a DBCrust upgrade that bumps the engine downloads them again. For
  machines without internet, copy the extension directory from one that has it.
- **`--read-only`, the AI agent and the broker refuse the whole thing**: a hardened connection
  never installs or loads an extension, so a DuckLake option is an error there.
- **Secrets**: the catalog string may not contain a password (`password=` is refused so it can
  never land in a session or a log); use `PGPASSWORD`/`~/.pgpass`, or a DuckDB `CREATE TEMPORARY
  SECRET` for object-store credentials typed in the session. Do not create persistent secrets:
  DuckDB stores them unencrypted under `~/.duckdb/stored_secrets`.
- **Without the option** you can still attach one by hand in a normal session:
  `INSTALL ducklake; ATTACH 'ducklake:meta.ducklake' AS lake (DATA_PATH 'data/'); USE lake;`
  (the same opt-in is yours to make, by typing it).
- S3 data paths need `httpfs` (`extensions=httpfs`), another downloaded extension, with its own
  TLS stack inside DuckDB — not DBCrust's.

The test suite attaches a real DuckLake end to end when `DUCKLAKE_TEST=1` is set (it downloads
the extension): `DUCKLAKE_TEST=1 cargo test -p dbcrust-driver-duckdb ducklake_round_trip`.

## Builds, wheels and CI

- **Cold build**: the engine's C++ amalgamation is compiled by the `cc` crate: about 15 CPU-minutes
  (4 minutes of wall time on 12 cores), and +19 MB in a release binary (figures in
  [Build features](/user-guide/development/#build-features)). Mr Boxington cannot cache that
  build script, so while hacking on the connector use a dedicated target directory
  (`CARGO_TARGET_DIR=.target-duckdb cargo test -p dbcrust-driver-duckdb`), where rebuilds are
  incremental. Set `CARGO_BUILD_JOBS` if a small machine runs out of memory.
- **Wheels and the desktop app** are built with `bundled`, which does not include `duckdb`, so
  they stay free of the C++ build. Build the app yourself with
  `cargo build -p dbcrust-gui --features duckdb`; the DuckDB entry then appears in the engine
  list. DuckDB does not target 32-bit platforms: the 32-bit Python check never enables it.
- **CI** has a dedicated `DuckDB` workflow (manual, weekly, and on changes to the connector)
  rather than a slot in the per-pull-request feature matrix, because even `cargo check` runs the
  C++ build script.

## Not supported yet

Writable agent aliases, DuckLake and extensions for agents, a DuckDB server mode (`duckdb -ui`
style sharing), encryption keys (`ENCRYPTION_KEY`), the Appender and the `COPY FROM` progress bar,
and Windows/musl/cross-compiled builds (untested). The engine is exercised against real
temporary databases in the test suite — no server, no mocks.

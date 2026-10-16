---
title: "URL Schemes & Shell Autocompletion"
---

# URL Schemes & Shell Autocompletion

DBCrust provides a comprehensive URL scheme system for connecting to databases with intelligent shell autocompletion support. This guide covers all supported connection methods and how to set up enhanced shell completion.

## 🔗 Supported URL Schemes

DBCrust supports database URLs, file-format URLs, bare local file paths, and special picker schemes optimized for common connection workflows:

### Standard Database Schemes


**PostgreSQL**


**Scheme:** `postgres://`

```bash
# Standard connection
dbcrust postgres://username:password@localhost:5432/database_name

# With SSL (recommended)
dbcrust postgres://username:password@localhost:5432/database_name?sslmode=require

# Alternative scheme (auto-converted to postgres://)
dbcrust postgres://username:password@localhost:5432/database_name
```

:::note[PostgreSQL Scheme Normalization]
Both `postgresql://` and `postgres://` are supported and automatically normalized to `postgres://` internally.
:::


**MySQL**


**Scheme:** `mysql://`

```bash
# Standard connection
dbcrust mysql://username:password@localhost:3306/database_name

# With SSL
dbcrust mysql://username:password@localhost:3306/database_name?ssl-mode=REQUIRED

# Custom port
dbcrust mysql://root:secret@mysql-server:3307/production
```


**SQLite**


**Scheme:** `sqlite://`

```bash
# Absolute path
dbcrust sqlite:///path/to/database.db

# Relative path
dbcrust sqlite://./myapp.db

# Memory database
dbcrust sqlite://:memory:

# Current directory
dbcrust sqlite://database.db
```


**ClickHouse**


**Scheme:** `clickhouse://`

```bash
# Standard connection (HTTP interface on port 8123)
dbcrust clickhouse://localhost:8123/default

# With credentials
dbcrust clickhouse://username:password@localhost:8123/database_name

# Remote ClickHouse server
dbcrust clickhouse://user:pass@clickhouse.company.com:8123/analytics

# Without authentication (when CLICKHOUSE_SKIP_USER_SETUP=1)
dbcrust clickhouse://localhost:8123/default
```

**Features:**
- Uses ClickHouse HTTP interface (port 8123) for optimal performance
- Support for ClickHouse-specific data types (UInt32, DateTime, etc.)
- Compatible with ClickHouse system tables and functions
- Automatic FORMAT handling for dynamic query results


**MongoDB**


**Schemes:** `mongodb://` or `mongodb+srv://`

```bash
# Standard MongoDB connection
dbcrust mongodb://user:password@localhost:27017/database_name

# MongoDB Atlas (SRV record)
dbcrust mongodb+srv://user:password@cluster.mongodb.net/database_name

# Without authentication (local development)
dbcrust mongodb://localhost:27017/myapp

# With connection options
dbcrust mongodb://user:pass@host:27017/db?authSource=admin&ssl=true

# Replica set connection
dbcrust mongodb://user:pass@host1:27017,host2:27017/db?replicaSet=myReplSet
```

**Features:**
- **SQL-like Query Interface**: Use familiar SELECT syntax that translates to MongoDB queries
- **Advanced Filtering**: LIKE patterns, IN operators, BETWEEN ranges, IS NULL checks, OR conditions
- **Database Management**: CREATE/DROP databases and collections via SQL syntax
- **Native MongoDB Commands**: Direct access to find, aggregate, and admin operations
- **Schema Inference**: Dynamic column detection from document structure
- **Text Search**: Full-text search capabilities with `\search` command
- **Index Management**: Create, drop, and list MongoDB indexes


**Elasticsearch**


**Schemes:** `elasticsearch://`, `elastic://`, `es://`

```bash
# Standard connection (HTTP interface on port 9200)
dbcrust elasticsearch://localhost:9200

# With authentication
dbcrust elasticsearch://elastic:password@localhost:9200

# Alternative schemes (all equivalent)
dbcrust elastic://localhost:9200
dbcrust es://localhost:9200

# With SSL (skip certificate verification for development)
dbcrust "elasticsearch://elastic:password@localhost:9200?ssl=true&verify_certs=false"

# Remote Elasticsearch cluster
dbcrust elasticsearch://user:pass@elasticsearch.company.com:9200

# Docker container
dbcrust docker://my-elasticsearch-container
```

**Features:**
- **Intelligent Index Handling**: Automatic quoting for index names with special characters (`logs-2024.01.01`)
- **Smart SELECT * Queries**: Auto-excludes array fields that cause SQL API failures
- **Comprehensive Field Mapping**: Shows nested fields (`author.login`) and multi-fields (`field.keyword`)
- **Field Capabilities Analysis**: Displays what operations each field supports (filter, search, agg, etc.)
- **SQL API Integration**: Uses Elasticsearch SQL for familiar query syntax
- **Container Auto-Discovery**: Detects Elasticsearch containers and excludes Kibana
- **Advanced Query Rewriting**: Handles complex queries with proper escaping

**White Dragon**

**Schemes:** `white-dragon://`, `whitedragon://`, `wd://`

```bash
# Local White Dragon HTTP server (default port 7700)
dbcrust white-dragon://127.0.0.1:7700

# TLS and reverse-proxy Basic authentication
dbcrust 'white-dragon://user:password@search.example.com:443?tls=true'
```

**Features:**
- SQL and qualifier-language search through `/v1/search`
- Exact generic WDSP v6 fields and index paths from schema API v3
- Stored mapping fields flattened into named result columns with SQL NULL for missing values
- Structured JSON policies for `named_field` sorting and generic column aggregations
- Safe use with `--read-only` because White Dragon exposes search-only operations

See the [White Dragon guide](/user-guide/white-dragon/) for examples.

**Redis / Valkey**

**Schemes:** `redis://`, `rediss://` (TLS), `valkey://`, `valkeys://`, `redis+unix://`, `unix://`

```bash
# Local server, database 0
dbcrust redis://localhost:6379

# ACL user, password and logical database 2 over TLS
dbcrust 'rediss://app:secret@cache.example.com:6380/2'

# Unix socket
dbcrust 'redis+unix:///run/redis.sock?db=1'
```

**Features:**
- Queries are `redis-cli` command lines (`HGETALL user:1`, `SCAN 0 MATCH user:* COUNT 100`); replies are mapped to tables by shape
- `\dt` lists keys with their type using a bounded `SCAN` (never `KEYS`), `\d key` shows type, TTL, memory, size and a sample
- `--read-only` accepts only a read-command allowlist and refuses `KEYS`, `FLUSHALL`, `CONFIG`, scripting and blocking commands
- Valkey, KeyDB and Dragonfly work unchanged

See the [Redis guide](/user-guide/redis/) for details.

**SQL Server / Azure SQL**

**Schemes:** `mssql://`, `sqlserver://`, `mssql+tls://`, `sqlserver+tls://`

```bash
# Local server (TLS on, certificate verified)
dbcrust 'mssql://sa:Passw0rd!@localhost:1433/master'

# Azure SQL with TDS 8.0 strict encryption
dbcrust 'sqlserver://app:secret@myserver.database.windows.net/mydb?encrypt=strict'

# Named instance (SQL Browser, UDP 1434): %5C is the backslash
dbcrust 'mssql://app:secret@dbhost%5CSQLEXPRESS/Sales'

# Development server with a self-signed certificate: explicit opt-in, logged as a warning
dbcrust 'mssql://sa:Passw0rd!@localhost/master?trustServerCertificate=true'
```

**Features:**
- T-SQL over SQL authentication; one pinned session (temp tables, `SET` options and transactions persist)
- `encrypt=true` (default, certificate verified) | `false` (login only) | `strict` (TDS 8.0); `ca_cert=` pins a CA
- Metadata from the catalog views (`\dt`, `\d table` with indexes, checks and foreign keys); `EXPLAIN` returns the estimated plan XML without running the statement
- `--read-only` accepts a single `SELECT` / `WITH … SELECT` and refuses procedures, linked servers, files, DDL and batches
- Statements past the timeout or Ctrl-C are cancelled on the server (TDS attention)

Windows / integrated and Azure AD sign-in are not supported yet. See the [SQL Server guide](/user-guide/sqlserver/) for details.

**Amazon Redshift**

**Schemes:** `redshift://`, `redshift+tls://`

```bash
# Provisioned cluster (TLS required by default, certificate not verified)
dbcrust 'redshift://awsuser:pass@my-cluster.abc123.eu-west-1.redshift.amazonaws.com:5439/dev'

# Serverless workgroup, certificate checked against the bundled Amazon roots
dbcrust 'redshift://analyst:pass@wg.123456789012.eu-west-1.redshift-serverless.amazonaws.com/dev?sslmode=verify-full'

# Through an SSH tunnel to a private cluster
dbcrust 'redshift://awsuser:secret@127.0.0.1:15439/dev?sslmode=disable'
```

**Features:**
- Redshift SQL over user and password; one pinned session (temp tables, `SET` and transactions persist)
- `sslmode=require` (default) | `verify-ca` | `verify-full` (+ `sslrootcert=`); `disable` / `allow` / `prefer` only for tunnels and tests
- Metadata from the `SVV_*` views, Spectrum schemas included (`\dt`, `\d table` with encodings, `DISTSTYLE` and `SORTKEY`); `EXPLAIN` returns the text plan
- `--read-only` accepts a single `SELECT` / `WITH … SELECT` / `SHOW` / `EXPLAIN` inside `BEGIN READ ONLY`
- Ctrl-C and the query timeout cancel on the server (`pg_cancel_backend`)

IAM authentication and the Redshift Data API are not supported yet. See the [Redshift guide](/user-guide/redshift/) for details.

**libSQL / Turso**

**Schemes:** `libsql://`, `turso://`, `libsql+http://`

```bash
# Turso Cloud (HTTPS, token as a query option; it is moved to the secret store, never saved in a URL)
dbcrust 'libsql://my-db-myorg.turso.io?authToken=eyJhbGciOi...'

# The token is asked for when the server rejects the connection
dbcrust 'turso://my-db-myorg.turso.io'

# A local sqld (plain HTTP is only accepted for this machine, or with an explicit tls=false)
dbcrust 'libsql+http://localhost:8080'

# sqld with namespaces
dbcrust 'libsql+http://localhost:8080/tenant1'
```

**Features:**
- SQLite SQL over Hrana-on-HTTP (JSON); each statement stateless, `BEGIN` ... `COMMIT` carried on the server's stream
- The auth token is the connection's password: secret store, bound to host, port, namespace and transport, redacted everywhere
- Metadata from `sqlite_schema` and the `pragma_*` functions (`\dt`, `\d table`); `EXPLAIN` returns `EXPLAIN QUERY PLAN` as a tree
- `--read-only` accepts a single read statement and asks the server (`describe`) before running it; a read-only token is the hard limit

Local files stay `sqlite:///path`; embedded replicas and the agent broker are not supported yet. See the [libSQL & Turso guide](/user-guide/libsql/) for details.

**InfluxDB**

**Schemes:** `influxdb://` (v1, InfluxQL), `influxdb2://` (v2, Flux), `influxdb3://` (v3, SQL); `influxdbs://`, `influxdb2s://`, `influxdb3s://` or `+https` for TLS

```bash
# v1: database in the path, login optional
dbcrust 'influxdb://reader:secret@localhost:8086/telemetry'

# v2: the API token goes where a password would, the organization is required
dbcrust 'influxdb2://:MY_TOKEN@localhost:8086/telemetry?org=acme'

# v3 (port 8181 by default), TLS
dbcrust 'influxdb3s://:MY_TOKEN@influx.example.com:8181/mydb'

# InfluxQL on a v2 server (DBRP mapping required), self-signed certificate: explicit opt-out
dbcrust 'influxdb2s://:MY_TOKEN@localhost/telemetry?org=acme&language=influxql&verify_certs=false'
```

**Features:**
- Language by generation: InfluxQL (v1), Flux (v2), SQL (v3); `?version=` on the generic scheme, `?language=influxql` on v2/v3
- The token lives in the password slot (`?token=` is moved there): stored in the secret store, redacted everywhere
- Replies are read while they arrive and stop at `max_rows` (10 000) / `max_bytes` (64 MiB)
- `\l`, `\dt`, `\d` from `SHOW …`, the Flux `schema` package or `information_schema`
- `--read-only` accepts reads only, lexed per language (no `INTO`, `to()`, `http`/`sql`/`secrets` imports…); InfluxDB 1.x does not enforce the `GET` split itself, so use a read-only credential

See the [InfluxDB guide](/user-guide/influxdb/) for details.

**Amazon S3, Cloudflare R2, MinIO** *(opt-in build: `--features aws`, Rust ≥ 1.94.1)*

**Schemes:** `s3://[KEY:SECRET@]bucket[/prefix]`, `r2://[KEY:SECRET@]account_id/bucket[/prefix]`, `minio://[KEY:SECRET@]host[:9000]/bucket[/prefix]`

```bash
# AWS: credentials from the environment, a profile (SSO, role, keys) or the instance
dbcrust 's3://my-bucket/logs/?region=eu-west-1'
dbcrust 's3://my-bucket?region=eu-west-1&profile=prod-ro' -c 'ls'

# Static keys (the secret key is the password: stored, bound to the endpoint, redacted)
dbcrust 's3://AKIA…:SECRET@my-bucket?region=eu-west-1'

# S3-compatible stores
dbcrust 'r2://KEY:SECRET@0123456789abcdef/media'
dbcrust 'minio://minioadmin:minioadmin@localhost:9000/data?tls=false'
dbcrust 's3://test:test@bucket?endpoint=http://localhost:4566&region=us-east-1'
```

**Features:**
- Seven typed commands, read-only by construction: `ls`, `head`, `get` (always a bounded `Range`, hard byte cap), `buckets`, `cd`, `pwd`, `help`
- Region: `?region=`, else an explicit `?profile=`'s, else the environment; `?endpoint=` and path-style addressing for non-AWS stores
- Credentials: `ACCESS:SECRET@`, an explicit `?profile=` (SSO, roles, `credential_process`), `?anonymous=true`, or the AWS default chain
- TLS is rustls on ring only (no aws-lc, no OpenSSL); `?ca_cert=` trusts a private CA
- `\l` buckets, `\dt` prefixes and objects at the current prefix, `\d KEY` metadata
- `--read-only` accepts every S3 command and refuses anything else; IAM is the server-side limit. The credential broker does not enrol S3 yet

See the [S3 guide](/user-guide/s3/) for details.

**Amazon DynamoDB** *(opt-in build: `--features aws` or `dynamodb`, Rust ≥ 1.94.1)*

**Schemes:** `dynamodb://[KEY:SECRET@][region][/table]`, `dynamodb+http://host[:port][/table]` (plain HTTP, DynamoDB Local), `ddb://` (alias)

```bash
# AWS: credentials from the environment, a profile (SSO, role, keys) or the instance
dbcrust dynamodb://eu-west-1 -c 'tables'
dbcrust 'dynamodb://eu-west-1/Orders?profile=prod-ro' -c 'query customer=acme --desc --limit 20'
dbcrust 'dynamodb://eu-west-1?profile=prod-ro' -c "SELECT * FROM \"Orders\" WHERE customer = 'acme'"

# Static keys (the secret key is the password: stored, bound to the endpoint and region, redacted)
dbcrust 'dynamodb://AKIA…:SECRET@eu-west-1/Orders'

# DynamoDB Local, LocalStack, a VPC endpoint
dbcrust 'dynamodb+http://local:local@localhost:8000?region=us-east-1'
dbcrust 'dynamodb://us-east-1?endpoint=http://localhost:4566'
```

**Features:**
- PartiQL `SELECT` and five typed commands: `tables`, `describe`, `scan`, `get`, `query`; **no writes**, even without `--read-only`
- Every read is bounded (a `Limit` never above the rows still wanted, a page budget) and the consumed capacity units are reported; a `scan` always warns
- Key values are converted to the key attribute's type from `DescribeTable` and checked before any read
- Items become rows: the union of attribute names (keys first), exact numbers, JSON for maps, lists and sets
- Credentials, region, `?endpoint=`, `?profile=`, `?role_arn=`, `?ca_cert=` as for S3; TLS is rustls on ring only
- `\l`/`\dt` tables, `\d` keys and indexes. The credential broker does not enrol DynamoDB yet

See the [DynamoDB guide](/user-guide/dynamodb/) for details.

**Amazon CloudWatch Logs** *(opt-in build: `--features aws` or `cloudwatch`, Rust ≥ 1.94.1)*

**Schemes:** `cloudwatch://[KEY:SECRET@][region][/log-group-or-prefix]`, `cloudwatch+http://host[:port]`, `cwlogs://` (alias)

```bash
dbcrust cloudwatch://eu-west-1 -c 'groups /aws/lambda/'
dbcrust 'cloudwatch://eu-west-1/aws/lambda/orders?profile=prod-ro' \
  -c 'since 30m fields @timestamp, @message | filter @message like /ERROR/ | sort @timestamp desc | limit 50'
dbcrust 'cloudwatch://eu-west-1?groups=/aws/lambda/a,/aws/lambda/b&since=15m&max_scan_gb=2'
```

**Features:**
- Logs Insights queries over a **mandatory time window** (`since 30m`, `between 2026-10-16 08:00 and now`; default the last hour, `?max_window=` caps it) on up to 50 log groups (`use`, `--group`, the URL path or `?groups=`)
- `StopQuery` on Ctrl-C, on timeout, on a `?max_scan_gb=` budget and on any failing poll; records matched, records scanned and bytes scanned are reported after each query
- `groups`, `streams`, `tail` (FilterLogEvents, oldest first, bounded); no write API is reachable
- Credentials and region as for S3 and DynamoDB; TLS is rustls on ring only
- `\l`/`\dt` log groups, `\d` group details and recent streams. The credential broker does not enrol CloudWatch Logs yet

See the [CloudWatch Logs guide](/user-guide/cloudwatch-logs/) for details.

**DuckDB** (opt-in build: `cargo install --path . --locked --features duckdb`)

**Schemes:** `duckdb://` (also `./file.duckdb` and `file.ddb`)

```bash
# A database file (created if it does not exist)
dbcrust duckdb:///data/warehouse.duckdb

# In memory: query Parquet/CSV/JSON in place
dbcrust 'duckdb://:memory:' -c "SELECT * FROM 'events.parquet' LIMIT 10"

# Read-only: the engine itself refuses writes, COPY TO, ATTACH, INSTALL and file functions
dbcrust duckdb:///data/warehouse.duckdb --read-only -c 'SELECT count(*) FROM orders'

# A DuckLake catalog (downloads the `ducklake` extension once; never without install_extensions=true)
dbcrust 'duckdb://:memory:?ducklake=sqlite:/srv/lake/meta.sqlite&data_path=/srv/lake/data/&install_extensions=true'
```

**Features:**
- The embedded engine on its own thread; Ctrl-C and the query timeout use DuckDB's `interrupt` and keep the session
- Options: `access_mode=read_only`, `memory_limit`, `threads`, `max_rows` (100 000), `extensions`, `install_extensions`, `ducklake`/`data_path`/`alias`, `allowed_directories` (read-only mode)
- `\l` lists the attached databases, `\c name` is `USE name`, `\dt` / `\d` read DuckDB's catalog; `EXPLAIN` is the text plan
- Nested types print as JSON, `DECIMAL`/`HUGEINT` keep every digit
- `--read-only` and the agent broker (local files only) are enforced by DuckDB: `access_mode=READ_ONLY`, external access off, configuration locked
- Without the `duckdb` feature the URL answers "DuckDB support is not compiled into this build"; it is no longer served by the Parquet/CSV/JSON engine

See the [DuckDB & DuckLake guide](/user-guide/duckdb/) for details.

### File Format Schemes

DBCrust can query file formats directly using Apache DataFusion, a powerful SQL query engine that operates on Parquet, CSV, and JSON files (a DuckDB build reads them natively too, through `duckdb://`). You can either use the explicit scheme or pass a local path with a known extension:

```bash
dbc sales.parquet
dbc './logs/*.csv?header=true'
dbc events.ndjson
dbc app.sqlite
```

Use `file://` to open an interactive picker for compatible files in the current directory:

```bash
dbc file://
```

**Safety limits for large files**

File-format connections run with conservative DataFusion safeguards to reduce the risk of an exploratory query against a huge Parquet/CSV/JSON file overwhelming your machine or terminal. Defaults include a 512 MiB DataFusion memory pool, at most 2 execution partitions, 256-row batches, a cooperative 60s query timeout, 10,000 returned rows, 2,048 displayed characters per cell, and 16 MiB of raw result text before dbcrust truncates with a warning row. Note: regular non-NDJSON JSON files may still be read once during registration to convert them to NDJSON.

You can raise limits for intentional heavy scans with URL parameters or environment variables:

| URL parameter | Environment variable | Default |
| --- | --- | --- |
| `datafusion_memory_limit_bytes` | `DBCRUST_DATAFUSION_MEMORY_LIMIT_BYTES` | `536870912` |
| `datafusion_target_partitions` | `DBCRUST_DATAFUSION_TARGET_PARTITIONS` | `min(available CPUs, 2)` |
| `datafusion_batch_size` | `DBCRUST_DATAFUSION_BATCH_SIZE` | `256` |
| `datafusion_max_query_seconds` | `DBCRUST_DATAFUSION_MAX_QUERY_SECONDS` | `60` |
| `datafusion_max_result_rows` | `DBCRUST_DATAFUSION_MAX_RESULT_ROWS` | `10000` |
| `datafusion_max_cell_chars` | `DBCRUST_DATAFUSION_MAX_CELL_CHARS` | `2048` |
| `datafusion_max_output_bytes` | `DBCRUST_DATAFUSION_MAX_OUTPUT_BYTES` | `16777216` |

Example:

```bash
dbc 'parquet:///data/warehouse.parquet?datafusion_max_query_seconds=180&datafusion_max_result_rows=50000'
```


**Parquet**


**Scheme:** `parquet://`

```bash
# Single Parquet file
dbcrust parquet:///data/sales_2024.parquet

# Multiple files with glob pattern
dbcrust 'parquet:///data/sales_*.parquet'

# Directory-based table name
dbcrust 'parquet:///data/warehouse/'
> SELECT * FROM warehouse LIMIT 10;
```

**Features:**
- **Columnar Storage Format**: Optimized for analytical queries with efficient compression
- **Full Schema Preservation**: Maintains complete type information including nested structures
- **Nested Field Support**: Query deeply nested Struct, Array, and Map types
- **Predicate Pushdown**: Filters applied at file level for optimal performance
- **Multi-Level Autocomplete**: Navigate nested fields like `data.exact_paths.auth/token/create`
- **Glob Pattern Support**: Query multiple files matching patterns

**Multi-Level Nested Field Navigation:**
```sql
-- Autocomplete shows all nested levels
SELECT data.[TAB] FROM sales
-- → data, data.customer, data.customer.address, data.customer.address.city

-- Navigate to any depth
SELECT data.customer.address.[TAB] FROM sales
-- → Shows: city, state, zip, country

-- Works with complex field names
SELECT data.exact_paths.[TAB] FROM policies
-- → Shows: auth/token/create, aws_okta/creds/management-ecr, ...
```


**CSV**


**Scheme:** `csv://`

```bash
# CSV with header row (default)
dbcrust csv:///data/users.csv

# Custom delimiter (tab-separated)
dbcrust 'csv:///data/data.tsv?delimiter=\t'

# No header row
dbcrust 'csv:///data/logs.csv?header=false'

# Multiple CSV files with glob pattern
dbcrust 'csv:///logs/*.csv?header=true'
> SELECT date, COUNT(*) FROM logs GROUP BY date;
```

**Query Parameters:**
- `?header=true|false` - CSV has header row (default: true)
- `?delimiter=,` - Field delimiter character (default: ',')

**Features:**
- **Schema Inference**: Automatic column type detection
- **Flexible Delimiters**: Support for CSV, TSV, and custom-delimited files
- **Glob Patterns**: Query multiple files matching `*.csv` patterns
- **Header Detection**: Configurable header row handling


**JSON**


**Scheme:** `json://`

```bash
# Standard JSON file
dbcrust json:///api_responses.json

# NDJSON (newline-delimited JSON)
dbcrust json:///events.ndjson

# JSON with nested structures
dbcrust json:///vault_policies.json
> SELECT data.exact_paths FROM res;
```

**Features:**
- **Automatic NDJSON Detection**: Recognizes and converts newline-delimited JSON
- **Deep Nested Field Support**: Navigate multi-level nested objects with autocomplete
- **Schema Inference**: Automatic type detection from JSON structure
- **Multi-Level Field Access**: Use dot notation like `data.field.subfield.value`
- **Complex Type Display**: Shows `Struct<N fields>` with expandable details

**Nested Field Schema Display:**
```
\d res

Table: res
Column           | Type
-----------------+--------------------
data             | Struct<4 fields>
timestamp        | Utf8
id               | Int64

Nested field details:
  data (Struct):
    - chroot_namespace: Utf8
    - exact_paths: Struct<25 fields>
    - glob_paths: Struct<10 fields>
    - root: Utf8

  data.exact_paths (Struct):
    - auth/token/create: Struct<1 fields>
    - auth/token/lookup-self: Struct<1 fields>
    ... and 23 more fields
```

**DataFusion SQL Features:**

All file formats support the full DataFusion SQL dialect:

- **Aggregate Functions**: `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`, `STDDEV`, `VAR`, `MEDIAN`, `APPROX_DISTINCT`
- **Window Functions**: `ROW_NUMBER`, `RANK`, `DENSE_RANK`, `LAG`, `LEAD`, `FIRST_VALUE`, `LAST_VALUE`
- **String Functions**: `CONCAT`, `UPPER`, `LOWER`, `SUBSTRING`, `TRIM`, `REPLACE`, `SPLIT_PART`, `LENGTH`
- **Date/Time Functions**: `NOW`, `CURRENT_DATE`, `DATE_TRUNC`, `EXTRACT`, `TO_TIMESTAMP`
- **Array Functions**: `ARRAY_AGG`, `ARRAY_LENGTH`, `ARRAY_CONTAINS`, `ARRAY_CONCAT`
- **Type Conversion**: `CAST`, `TRY_CAST`

**Cross-Format Queries:**

You can JOIN data across different file formats:

```sql
-- Join Parquet and CSV data
SELECT
    u.name,
    o.total
FROM 'users.parquet' u
JOIN 'orders.csv' o ON u.id = o.user_id
WHERE o.total > 100;

-- Combine JSON and Parquet
SELECT
    p.data.customer.name,
    s.amount
FROM 'policies.json' p
JOIN 'sales.parquet' s ON p.id = s.policy_id;
```

### Advanced Connection Schemes


**Docker Containers**


**Scheme:** `docker://`

```bash
# Interactive container selection
dbcrust docker://
# → Shows list of running database containers

# Direct container connection
dbcrust docker://postgres-container
dbcrust docker://my-mysql-db

# With credentials and database
dbcrust docker://user:pass@container-name/database

# Examples
dbcrust docker://postgres-dev
dbcrust docker://app_user:secret@mysql-prod/app_db
```

**Features:**
- Automatic discovery of running database containers
- Support for PostgreSQL, MySQL, SQLite, ClickHouse, and MongoDB containers
- OrbStack integration on macOS
- Intelligent port mapping and network resolution
- Special handling for ClickHouse containers with `CLICKHOUSE_SKIP_USER_SETUP=1`
- MongoDB container detection for mongo, mongodb, and bitnami/mongodb images


**Saved Sessions**


**Scheme:** `session://`

```bash
# Interactive session selection
dbcrust session://
# → Shows list of saved sessions

# Direct session connection
dbcrust session://production_db
dbcrust session://staging_postgres
dbcrust session://local_dev
```

**Session Management:**
```bash
# Save current connection as a session
\ss production_db

# Save with a dynamic password command (stdout becomes the password on reconnect)
\ss chpprd --password-command vault kv get -mount=secret -field=password preprod/gim/admin/clickhouse/default-user

# List all saved sessions
\s

# Delete a session
\sd old_session

# Connect to specific session
\s production_db
```


**Recent Connections**


**Scheme:** `recent://`

```bash
# Interactive recent connection selection
dbcrust recent://
# → Shows numbered list of recent connections
#   1. postgres://user@localhost:5432/mydb
#   2. docker://postgres-dev/testdb
#   3. mysql://root@mysql-server:3306/app
```

**Recent Connection Management:**
```bash
# List recent connections
\r

# Clear recent connection history
\rc
```


**Generic File Picker**


**Scheme:** `file://`

```bash
# Pick a compatible file from the current directory
dbcrust file://

# Pick from a specific directory
dbcrust file:///data/exports

# Open a specific file while still inferring its backend
dbcrust file://./sales.csv
```

`file://` lists compatible Parquet, CSV/TSV, JSON/NDJSON, and SQLite files using the interactive prompt. If you already know the file path, you can usually omit the scheme entirely (`dbc sales.csv`, `dbc app.sqlite`).


**HashiCorp Vault**


**Schemes:** `vault://` or `vaultdb://` (deprecated)

```bash
# Full vault URL
dbcrust vault://role-name@mount-path/database-path

# Interactive vault connection
dbcrust vault://
# → Prompts for role, mount, and database

# Alternative scheme (deprecated)
dbcrust vault://app-role@database/postgres-prod
```

**Configuration:**
```toml
[vault]
addr = "https://vault.company.com"
token_file = "~/.vault-token"
```

## 🚀 Shell Autocompletion

DBCrust provides intelligent shell autocompletion that understands URL schemes and provides contextual suggestions.

### Installation


**Bash**


```bash
# Generate completion script
dbcrust --completions bash > ~/.local/share/bash-completion/completions/dbcrust

# Or install system-wide
sudo dbcrust --completions bash > /etc/bash_completion.d/dbcrust

# Reload your shell
source ~/.bashrc
```


**Zsh**


```bash
# Create completions directory if it doesn't exist
mkdir -p ~/.local/share/zsh/site-functions

# Generate completion script
dbcrust --completions zsh > ~/.local/share/zsh/site-functions/_dbcrust

# Add to your .zshrc if not already present
echo 'fpath=(~/.local/share/zsh/site-functions $fpath)' >> ~/.zshrc
echo 'autoload -Uz compinit && compinit' >> ~/.zshrc

# Reload your shell
source ~/.zshrc
```


**Fish**


```bash
# Generate completion script
dbcrust --completions fish > ~/.config/fish/completions/dbcrust.fish

# Reload fish completions
fish -c "complete --erase --command dbcrust; source ~/.config/fish/completions/dbcrust.fish"
```


**PowerShell**


```powershell
# Generate completion script
dbcrust --completions powershell > $PROFILE.CurrentUserAllHosts.Replace("profile.ps1", "Completions/dbcrust.ps1")

# Add to your PowerShell profile
Add-Content $PROFILE.CurrentUserAllHosts '. $PSScriptRoot/Completions/dbcrust.ps1'
```

### Autocompletion Features

#### URL Scheme Completion

Type a partial scheme and press **TAB** to see available options:

```bash
dbc pos[TAB] → postgres://
dbc my[TAB]  → mysql://
dbc sq[TAB]  → sqlite://
dbc cl[TAB]  → clickhouse://
dbc mo[TAB]  → mongodb://
dbc par[TAB] → parquet://
dbc cs[TAB]  → csv://
dbc js[TAB]  → json://
dbc doc[TAB] → docker://
dbc ses[TAB] → session://
dbc rec[TAB] → recent://
dbc va[TAB]  → vault://
```

#### Contextual Completions

DBCrust provides smart contextual completions based on the URL scheme:


**Docker Containers**


```bash
# Shows running database containers
dbc docker://[TAB]
# → postgres-dev mysql-test clickhouse-analytics mongodb-cache

dbc docker://post[TAB] → docker://postgres-dev
dbc docker://my[TAB]   → docker://mysql-test
dbc docker://cl[TAB]   → docker://clickhouse-analytics
dbc docker://mo[TAB]   → docker://mongodb-cache
```

**How it works:**
- Queries Docker API for running containers
- Filters for database containers (PostgreSQL, MySQL, SQLite, ClickHouse, MongoDB images)
- Only shows containers that are currently running
- Matches container names that start with your input


**Saved Sessions**


```bash
# Shows your saved sessions
dbc session://[TAB]
# → production_db staging_postgres local_dev

dbc session://prod[TAB] → session://production_db
dbc session://loc[TAB]  → session://local_dev
```

**How it works:**
- Reads from `~/.config/dbcrust/sessions.toml`
- Shows all saved session names
- Matches sessions that start with your input


**SQLite and generic file picker**


```bash
# Delegates to shell file completion
dbc sqlite://[TAB]
dbc file://[TAB]
# → Uses your shell's built-in file completion

dbc sqlite://./[TAB] → sqlite://./myapp.db sqlite://./test.db
```

#### Complete Examples

```bash
# Scheme completion
dbc [TAB]
# → postgres:// mysql:// sqlite:// docker:// file:// session:// recent:// vault://

# Docker container completion
dbc docker://[TAB]
# → postgres-dev mysql-prod clickhouse-analytics mongodb-cache

# Session completion
dbc session://[TAB]
# → production staging development local

# Recent connection (interactive)
dbc recent://[ENTER]
# → 1. postgres://user@localhost:5432/mydb
#   2. docker://postgres-dev/testdb
#   3. mysql://root@server:3306/app
#   Select connection [1-3]:
```

## 🔧 Advanced Configuration

### SSH Tunnel Patterns

Configure automatic SSH tunnels based on hostname patterns:

```toml
[ssh_tunnel_patterns]
"^db\\.internal\\..*\\.com$" = "user@jumphost.example.com:2222"
"^.*\\.prod\\.company\\.com$" = "deploy@bastion.company.com"
```

When connecting to a matching hostname, DBCrust automatically establishes an SSH tunnel:

```bash
# This automatically uses the SSH tunnel
dbcrust postgres://app@db.internal.example.com:5432/prod
# → Tunnels through user@jumphost.example.com:2222
```

### Default URL Handling

URLs without schemes default to PostgreSQL:

```bash
# These are equivalent
dbcrust localhost:5432/mydb
dbcrust postgres://localhost:5432/mydb
```

### Connection History

All successful connections are automatically saved to recent connection history:

```toml
[[recent_connections]]
connection_url = "postgres://user@localhost:5432/testdb"
display_name = "user@localhost:5432/testdb"
timestamp = "2024-01-15T14:22:33Z"
database_type = "PostgreSQL"
success = true

[[recent_connections]]
connection_url = "docker://postgres-dev/myapp"
display_name = "docker://postgres-dev/myapp"
timestamp = "2024-01-15T14:20:15Z"
database_type = "PostgreSQL"
success = true
```

## 🎯 Best Practices

### Session Management

1. **Save frequently used connections as sessions:**
   ```bash
   # Connect to production
   dbcrust postgres://readonly@prod.db.company.com:5432/analytics

   # Save as session
   \ss prod_analytics

   # Later, reconnect easily
   dbcrust session://prod_analytics
   ```

2. **Use meaningful session names:**
   ```bash
   \ss prod_readonly      # Good: describes environment and access
   \ss staging_full       # Good: describes environment and permissions
   \ss db1                # Bad: not descriptive
   ```

### Docker Workflows

1. **Use interactive mode for exploration:**
   ```bash
   # Explore available containers
   dbcrust docker://
   ```

2. **Use direct connection for automation:**
   ```bash
   # Script-friendly (no interaction)
   dbcrust docker://postgres-prod/analytics -c "SELECT COUNT(*) FROM users"
   ```

### URL Scheme Selection

Choose the right scheme for your use case:

| Use Case | Recommended Scheme | Example |
|----------|-------------------|---------|
| Local development | `postgres://`, `mysql://`, `sqlite://`, `mongodb://` | `postgres://localhost:5432/dev` |
| Production access | `session://` or `vault://` | `session://prod_readonly` |
| Container development | `docker://` | `docker://postgres-dev` |
| Quick reconnection | `recent://` | `recent://` |
| Team sharing | `session://` with shared config | `session://shared_staging` |
| Document databases | `mongodb://` or `mongodb+srv://` | `mongodb://localhost:27017/app` |

## 🔍 Troubleshooting

### Autocompletion Issues

**Completions not working?**
```bash
# Check if completion script is installed
ls ~/.local/share/bash-completion/completions/dbcrust  # Bash
ls ~/.zfunc/_dbcrust                                     # Zsh
ls ~/.config/fish/completions/dbcrust.fish             # Fish

# Regenerate completion scripts
dbcrust --completions bash > ~/.local/share/bash-completion/completions/dbcrust
dbcrust --completions zsh > ~/.zfunc/_dbcrust           # Zsh
dbc --completions zsh > ~/.zfunc/_dbc                   # Zsh (dbc binary)

# Test basic completion
type _usage_complete_dbcrust  # Should show function definition
```

**Docker completions not showing containers?**
```bash
# Check Docker connectivity
docker ps --format '{{.Names}}' | grep -E 'postgres|mysql|mariadb|sqlite|mongo|clickhouse'

# Check Docker permissions
docker info  # Should not require sudo
```

### Connection Issues

**Session not found?**
```bash
# Check available sessions
\s

# Check session file
cat ~/.config/dbcrust/sessions.toml
```

**Docker connection failed?**
```bash
# Check if container is running
docker ps | grep container-name

# Check container database type
docker inspect container-name | grep -i image
```

### Performance

**Autocompletion feels slow?**

DBCrust caches autocompletion data for performance. If you notice slow completions:

1. **Docker completions** cache running containers for 30 seconds
2. **Session completions** read from disk but are very fast
3. **Scheme completions** are instant (hardcoded)

## 🧠 Interactive SQL Autocompletion

Once connected to a database, DBCrust provides advanced SQL autocompletion within the interactive session. This is separate from shell completion and works by understanding SQL syntax context.

### Context-Aware SQL Completion

DBCrust analyzes your SQL statement in real-time to provide relevant suggestions based on the SQL clause you're in:

#### SELECT Context Intelligence

```sql
-- After SELECT keyword, suggests aggregates and wildcards
SELECT [TAB]
-- Suggestions: *, COUNT(, SUM(, AVG(, MAX(, MIN(, DISTINCT

-- Column suggestions work when table is visible before cursor
SELECT * FROM users WHERE [TAB]
-- Suggestions: id, name, email, created_at, status
```

#### WHERE Clause Precision

```sql
-- After WHERE, suggests ONLY column names (no functions or tables)
SELECT * FROM users WHERE [TAB]
-- Suggestions: id, name, email, created_at, status, active
-- NOT suggested: users, orders, *, COUNT(

-- Understands multiple tables in FROM clause
SELECT * FROM users u JOIN orders o ON u.id = o.user_id WHERE [TAB]
-- Suggestions: columns from BOTH users and orders tables
```

#### FROM Clause Behavior

```sql
-- After FROM, suggests table names (preserves existing behavior)
SELECT * FROM [TAB]
-- Suggestions: users, orders, products, categories
-- NOT suggested: column names, functions, or wildcards
```

### Advanced SQL Context Support

#### ORDER BY and GROUP BY

```sql
-- Suggests column names from tables in FROM clause
SELECT * FROM users ORDER BY [TAB]
-- Suggestions: id, name, email, created_at, status

SELECT COUNT(*) FROM orders GROUP BY [TAB]
-- Suggestions: status, user_id, product_id, created_at
```

#### HAVING Clause

```sql
-- Suggests both aggregate functions AND column names
SELECT status, COUNT(*) FROM users GROUP BY status HAVING [TAB]
-- Suggestions: COUNT(, SUM(, AVG(, MAX(, MIN(, status
```

#### Complex Query Support

```sql
-- Handles complex multi-table scenarios
SELECT u.name, o.total
FROM users u
  LEFT JOIN orders o ON u.id = o.user_id
  JOIN products p ON o.product_id = p.id
WHERE [TAB]
-- Suggests: u.id, u.name, u.email, o.id, o.total, o.status, p.name, p.price
```

### Schema-Based Completions

#### Table Names

```sql
-- Prefix matching for table names
SELECT * FROM us[TAB]
-- Suggestions: users, user_sessions, user_preferences
```

#### Column Names with Dot Notation

```sql
-- After table.dot, suggests columns from that specific table
SELECT users.[TAB] FROM users
-- Suggestions: id, name, email, created_at, status, active

-- Works with table aliases
SELECT u.[TAB] FROM users u
-- Suggestions: id, name, email, created_at, status, active
```

#### SQL Keywords

```sql
-- Keyword expansion
SEL[TAB] → SELECT
FR[TAB] → FROM
WH[TAB] → WHERE

-- Full statement completion
SEL[TAB] name FR[TAB] users WH[TAB] active = true
-- Expands to: SELECT name FROM users WHERE active = true
```

### Backslash Command Completion

DBCrust also provides completion for backslash commands:

#### Named Queries

```sql
-- After \n, suggests named query names
\n [TAB]
-- Suggestions: active_users, monthly_report, user_orders

-- After \nd, suggests named queries to delete
\nd [TAB]
-- Suggestions: old_report, unused_query
```

#### Session Management

```sql
-- After \s, suggests saved session names
\s [TAB]
-- Suggestions: production, staging, development, local

-- After \sd, suggests sessions to delete
\sd [TAB]
-- Suggestions: old_staging, temp_connection
```

### Performance & Caching

- **Real-time Context Analysis**: SQL parsing happens instantly with no noticeable delay
- **Schema Caching**: Table and column metadata is cached for fast repeated access
- **Smart Invalidation**: Cache automatically refreshes when database schema changes
- **Background Loading**: Schema data loads proactively for better responsiveness

### Technical Features

- **Multi-Database Support**: Works with PostgreSQL, MySQL, SQLite, ClickHouse, and MongoDB
- **Case-Insensitive Matching**: Completions work regardless of case
- **Backwards Compatible**: All existing completion behavior is preserved
- **Error Recovery**: Completion works even with partial or incomplete SQL statements

:::tip[Completion Best Practices]

- **Use TAB frequently** - completion works at any point in your SQL statement
- **Trust the context** - suggestions are filtered to be relevant to your current clause
- **Mix with schema exploration** - use `\dt` to see tables, then use completion for columns
- **Leverage named queries** - save complex queries and use `\n [TAB]` to find them quickly
:::

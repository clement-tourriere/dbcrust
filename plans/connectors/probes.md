# Probe projects (reproducibility)

Each probe = `[dependencies] tokio = { version = "1", default-features = false, features = ["rt","macros","time","net"] }` + the lines below, `Cargo.lock` copied from the repo root lock of 2026-10-15, DBCrust release profile, built with `CARGO_TARGET_DIR=<fresh> ~/.cargo/bin/cargo build --release`. New-crate counts: `cargo tree --prefix none -e normal` pairs minus the lock's `(name, version)` pairs.

## aws_all

```toml
aws-config = { version = "=1.12.0", default-features = false, features = ["rt-tokio", "behavior-version-latest", "sso", "credentials-process"] }
aws-smithy-http-client = { version = "=1.5.0", default-features = false, features = ["rustls-ring"] }
aws-sdk-s3 = { version = "=1.152.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
aws-sdk-dynamodb = { version = "=1.130.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
aws-sdk-cloudwatchlogs = { version = "=1.156.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
```

## aws_config

```toml
aws-config = { version = "=1.12.0", default-features = false, features = ["rt-tokio", "behavior-version-latest", "sso", "credentials-process"] }
aws-smithy-http-client = { version = "=1.5.0", default-features = false, features = ["rustls-ring"] }
```

## aws_cwl

```toml
aws-config = { version = "=1.12.0", default-features = false, features = ["rt-tokio", "behavior-version-latest", "sso", "credentials-process"] }
aws-smithy-http-client = { version = "=1.5.0", default-features = false, features = ["rustls-ring"] }
aws-sdk-cloudwatchlogs = { version = "=1.156.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
```

## aws_ddb

```toml
aws-config = { version = "=1.12.0", default-features = false, features = ["rt-tokio", "behavior-version-latest", "sso", "credentials-process"] }
aws-smithy-http-client = { version = "=1.5.0", default-features = false, features = ["rustls-ring"] }
aws-sdk-dynamodb = { version = "=1.130.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
```

## aws_default

```toml
aws-config = { version = "=1.12.0" }
aws-sdk-s3 = { version = "=1.152.0" }
```

## aws_min

```toml
aws-config = { version = "=1.12.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
aws-smithy-http-client = { version = "=1.5.0", default-features = false, features = ["rustls-ring"] }
aws-sdk-s3 = { version = "=1.152.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
```

## aws_s3

```toml
aws-config = { version = "=1.12.0", default-features = false, features = ["rt-tokio", "behavior-version-latest", "sso", "credentials-process"] }
aws-smithy-http-client = { version = "=1.5.0", default-features = false, features = ["rustls-ring"] }
aws-sdk-s3 = { version = "=1.152.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
```

## base

```toml
# (tokio only)
```

## duckdb_bundled

```toml
duckdb = { version = "=1.10506.0", features = ["bundled"] }
```

## duckdb_dyn

```toml
duckdb = { version = "=1.10506.0" }
```

## duckdb_full

```toml
duckdb = { version = "=1.10506.0", features = ["bundled", "parquet", "json"] }
```

## influx

```toml
influxdb = { version = "=0.8.0", default-features = false, features = ["serde", "rustls"] }
```

## influx_np

```toml
influxdb = { version = "=0.8.0", default-features = false, features = ["serde"] }
reqwest = { version = "0.13", default-features = false, features = ["rustls-no-provider"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std"] }
```

## libsql

```toml
libsql = { version = "=0.10.0", default-features = false, features = ["remote", "tls"] }
```

## mssql

```toml
tiberius = { version = "=0.13.0", default-features = false, features = ["rustls", "tds73", "sql-browser-tokio", "chrono", "rust_decimal"] }
tokio-util = { version = "0.7", features = ["compat"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std"] }
```

## mssql012

```toml
tiberius = { version = "=0.12.3", default-features = false, features = ["rustls", "tds73", "sql-browser-tokio", "chrono", "rust_decimal"] }
tokio-util = { version = "0.7", features = ["compat"] }
```

## mssql_patched

```toml
tiberius = { path = "<vendored tiberius 0.13.0 + patch, see mssql.md>", default-features = false, features = ["rustls", "tds73", "sql-browser-tokio", "chrono", "rust_decimal"] }
tokio-util = { version = "0.7", features = ["compat"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std"] }
```

## objstore

```toml
object_store = { version = "=0.13.2", default-features = false, features = ["aws"] }
futures = "0.3"
```

## redis

```toml
redis = { version = "=1.7.1", default-features = false, features = ["tokio-rustls-comp", "tls-rustls-webpki-roots", "connection-manager", "streams"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std"] }
```

## redis_cluster

```toml
redis = { version = "=1.7.1", default-features = false, features = ["tokio-rustls-comp", "tls-rustls-webpki-roots", "connection-manager", "streams", "cluster-async", "sentinel"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std"] }
```

## turso

```toml
turso_serverless = { version = "=0.1.3" }
```

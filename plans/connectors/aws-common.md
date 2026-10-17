# Shared AWS foundation (`aws` base for `s3`, `dynamodb`, `cloudwatch`; later Redshift IAM)

Read this before `s3.md`, `dynamodb.md`, `cloudwatch.md`. One workspace crate (`dbcrust-aws`, feature `aws`) owns configuration, credentials, the ring HTTP client and limits; the three service drivers depend on it.

## 1. Crates and versions (all >= 24 h old on 2026-10-15; every SDK crate is published by `aws-sdk-rust-ci` / team `github:awslabs:rust-sdk-owners`, Apache-2.0)

| Crate | Pin | Published | Age |
|---|---|---|---|
| `aws-config` | `=1.12.0` | 2026-09-04 | 41 d |
| `aws-smithy-http-client` | `=1.5.0` | 2026-09-30 | 15 d |
| `aws-sdk-s3` | `=1.152.0` | 2026-10-01 | 14 d |
| `aws-sdk-dynamodb` | `=1.130.0` | 2026-10-01 | 14 d |
| `aws-sdk-cloudwatchlogs` | `=1.156.0` | 2026-10-01 | 14 d |
| (transitive, resolved) `aws-sdk-sts 1.119.0`, `aws-sdk-sso 1.114.0`, `aws-sdk-ssooidc 1.116.0`, `aws-smithy-runtime 1.16.0`, `aws-smithy-types 1.8.1`, `aws-runtime 1.10.0` | by cargo | 2026-09 / 2026-10 | youngest resolved non-AWS crate: `tokio-util 0.7.20` (2026-10-09) |

The SDK publishes every few days; re-resolve with `cargo update -p <crate> --precise` and run `.github/scripts/check_cargo_cooldown.py --base HEAD` on every bump. The 24 h rule is satisfied for the whole probe tree (checked against the crates.io API: 0 crates < 24 h among the 34-36 new pairs).

**MSRV: all AWS crates declare `rust-version = 1.94.1`**; DBCrust declares 1.91. Enabling the `aws` feature (or the whole workspace) forces an MSRV bump to 1.94.1 (the toolchain used for these probes was 1.96.0). Raise `rust-version` for the workspace or keep the AWS crates in a member crate whose own `rust-version` is 1.94.1 and exclude it from the 1.91 CI job.

RustSec: `cargo audit` on the all-three probe lock shows nothing beyond the shared `rustls 0.23.45` baseline (RUSTSEC-2026-0346..0349, fix 0.23.46 published 2026-10-14). No OpenSSL/native-tls anywhere in the tree.

## 2. TLS: ring only, with no default features

The SDK's defaults pull aws-lc-rs (`aws-smithy-runtime/default-https-client` -> `aws-smithy-http-client/rustls-aws-lc`, plus `prefer-post-quantum`). Measured: `aws-config` + `aws-sdk-s3` with **default features** = **54** new crate pairs, `aws-lc-rs 1.18.1`/`aws-lc-sys 0.45.0` (cmake C build) on the path from `aws-smithy-http-client`. The ring configuration (**measured, builds and audits clean**):

```toml
aws-config = { version = "=1.12.0", default-features = false, features = [
    "rt-tokio", "behavior-version-latest",
    # opt-in extras, each adds crates: "sso" (aws-sdk-sso + ssooidc), "credentials-process" (tokio/process)
] }
aws-smithy-http-client = { version = "=1.5.0", default-features = false, features = ["rustls-ring"] }  # rustls 0.23 + ring, hyper 1.x
aws-sdk-s3 = { version = "=1.152.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
aws-sdk-dynamodb = { version = "=1.130.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
aws-sdk-cloudwatchlogs = { version = "=1.156.0", default-features = false, features = ["rt-tokio", "behavior-version-latest"] }
```

Because no default HTTP client is compiled, DBCrust **must inject one** for every SDK config (otherwise the SDK fails at the first request with a "no HTTP client" error):

```rust
use aws_smithy_http_client::{tls, Builder};
let http = Builder::new()
    .tls_provider(tls::Provider::Rustls(tls::rustls_provider::CryptoMode::Ring))
    .build_https();
let cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
    .http_client(http)
    /* .region(..) .endpoint_url(..) .credentials_provider(..) */
    .load().await;
```

Notes: do **not** use the SDK feature `rustls` / `legacy-https-client` (`aws-smithy-runtime/tls-rustls` -> `legacy-rustls-ring` = hyper 0.14 + rustls 0.21 + webpki 0.101, the advisory-ridden old stack). `aws-smithy-http-client` uses hyper-rustls 0.27 / hyper 1.x / rustls 0.23 - all already in DBCrust's lock, which is why only ~29 crates are new. Root store = `rustls-native-certs` (OS roots) inside smithy's default client.

## 3. Measured weight (DBCrust release profile; clean builds on a 12-core Mac, serialized; "CPU" = user+sys)

| Probe | New crate pairs vs lock | Clean build wall / CPU | Stripped binary | Delta vs tokio-only (389 KB) |
|---|---|---|---|---|
| `aws-config` only (sso + credentials-process + ring client) | **29** (27 new names + `itoa`, `ryu` bumps) | 23 s wall / 120 s CPU | 3.81 MB | +3.44 MB |
| + `aws-sdk-s3` | **34** (+5) | 46 s wall / 159 s CPU | 4.11 MB | +3.74 MB |
| + `aws-sdk-dynamodb` | **31** (+2) | 34 s wall / 136 s CPU | 4.03 MB | +3.66 MB |
| + `aws-sdk-cloudwatchlogs` | **32** (+3) | 48 s wall / 156 s CPU | 3.97 MB | +3.60 MB |
| all three | **36** | 51 s wall / 216 s CPU | 4.37 MB | +4.00 MB |
| `aws-config` minimal (no `sso`, no `credentials-process`) + s3 | 32 | 58 s wall / 159 s CPU | 3.69 MB | +3.32 MB |
| *default features* config + s3 (aws-lc, post-quantum) | 54 | n/a (resolved, not built: aws-lc-sys C) | n/a | n/a |

Marginal cost of each service SDK is small after the shared base: the three services together add ~7 crates and ~0.6 MB. Compile time is dominated by the large generated client crates (s3 ~45 s).

Version drift note: resolving the probes bumped `itoa 1.0.18`, `ryu 1.0.23`, `tokio-util 0.7.16 -> 0.7.20` in the lock (all published > 24 h before this measurement).

## 4. Credentials, regions and the secret store

- Preferred credentials are **ambient** (SSO / profile / web identity / IMDS / ECS): nothing secret is stored by DBCrust, only the `profile` name or `role_arn`. Static keys are supported through the secret store: `access_key_id` (identifier, non-secret but part of the `TargetBinding.user`), `secret_access_key` and `session_token` are secrets (one record, three named fields; the record model in G.2/O3 is `secret: String`, so store a small JSON/KV blob or three `conn:aws:...:field` records - decide when the store API is final).
- `ConnectionInfo` fields common to all AWS connectors: `host` = custom endpoint host (MinIO, R2, LocalStack, VPC endpoint) or None for AWS, `port`, `username` = `access_key_id` or `profile`, `password` = `secret_access_key`, `options`: `region`, `endpoint`, `profile`, `role_arn`, `external_id`, `session_name`, `path_style`, `session_token` (secret). Add `session_token`, `secret_access_key`, `aws_secret_access_key` to the redaction keys.
- **Broker (service mode) caveats** (G.4 "no env overrides"): the SDK default chain reads `AWS_*` env, `~/.aws/{config,credentials}` and `AWS_ENDPOINT_URL*` - an agent that controls the broker's environment or home could redirect or substitute credentials. In the gateway build `SdkConfig` explicitly (`region`, `endpoint_url`, `credentials_provider` = static or profile-by-name from the service user's own `AWS_CONFIG_FILE`), set `profile_files` explicitly, and never inherit `AWS_ENDPOINT_URL*`/`AWS_PROFILE` from the client environment. Disable `credentials-process` in the broker build (it executes a command named in a config file).
- `TargetBinding`: engine=`s3|dynamodb|cloudwatch`, host=region (or custom endpoint), port, database=bucket / table prefix / log-group, user=`access_key_id` / profile / role ARN, tls_mode. A secret may not be used with a different region/endpoint (prevents SigV4 requests to an attacker endpoint with a valid signature header exposing the access key id).

## 5. Common safety notes

- Read-only is **IAM**, not client-provable (`iam:SimulatePrincipalPolicy` is rarely granted; dbflux keeps `WritePrivilege::Unknown` for all three). Use the proposed `ReadOnlyKind::ClientStructural` (README): the agent surface exposes typed read operations only, so no write API call is reachable, while `native_read_only` stays `false` unless the owner supplies attestation + the optional `SimulatePrincipalPolicy` probe passes.
- Cancellation: dropping the SDK future aborts the HTTP request (native), but already-started server work (DynamoDB scan, CloudWatch Logs Insights) continues unless explicitly stopped (`StopQuery` for CloudWatch).
- Costs: S3 GET/egress, DynamoDB RCUs, CloudWatch Logs Insights (billed per GB scanned) are real money. Every driver must expose `max_bytes`/`max_pages`/time-window caps and report consumed units in result notices.
- Test harness shared by all three: LocalStack (`localstack/localstack`, community tier supports S3, DynamoDB, Logs; not CloudWatch metrics) + `AWS_ENDPOINT_URL`, MinIO for S3, `amazon/dynamodb-local` for DynamoDB. SDK unit tests use `aws-smithy-http-client`'s `test-util` `StaticReplayClient` as a dev-dependency only.

## 6. Effort and gating

- Shared `dbcrust-aws` base: **M** (3-4 days) - config builder, ring client, retry/timeout config, region/endpoint resolution, redaction, error mapping, fake-client test support.
- Feature `aws` (hidden, enabled by `s3`/`dynamodb`/`cloudwatch`): **opt-in** per D3 (heavy: +~3.6 MB, +29 crates, MSRV 1.94.1).

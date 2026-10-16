//! Shared AWS foundation for the DBCrust AWS connectors (S3 today; DynamoDB and
//! CloudWatch Logs next). Not a driver: it owns what every service client needs
//! and nothing service-specific.
//!
//! - [`AwsSettings`]: region, profile, custom endpoint, path style, anonymous
//!   access, static credentials and timeouts, read from a [`ConnectionInfo`]
//!   (`AwsSettings::from_connection_info`). Static credentials are secrets: the
//!   `Debug` output and every error mask them.
//! - [`CredentialSource`]: which credentials a connection uses, decided from
//!   the settings alone (a pure function, see [`AwsSettings::credential_source`]):
//!   static keys, anonymous, an explicit profile, or the SDK default chain
//!   (environment → shared config / SSO → web identity → ECS → IMDS).
//! - [`load_sdk_config`]: the `SdkConfig` the service clients are built from.
//! - [`http`]: the HTTP client, **rustls on ring only**. The SDK's own default
//!   client pulls `aws-lc-rs` (a C build); every AWS crate here is compiled with
//!   `default-features = false` and this client is injected into each
//!   configuration instead, so a build with the AWS connectors never contains
//!   aws-lc or OpenSSL (`cargo tree -i aws-lc-rs` stays empty).
//! - [`redact`]: renders an SDK error chain without ever echoing a secret.
//!
//! Needs Rust 1.94.1 (the AWS crates' MSRV); the rest of the workspace does not.

mod config;
mod error;
pub mod http;
mod redact;
mod settings;

pub use aws_credential_types::provider::SharedCredentialsProvider;
pub use aws_types::SdkConfig;
pub use aws_types::region::Region;
pub use config::load_sdk_config;
pub use error::AwsError;
pub use redact::{redact_error, redact_message};
pub use settings::{AwsSettings, CredentialSource, StaticCredentials};

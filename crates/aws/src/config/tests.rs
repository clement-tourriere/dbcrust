//! Credential and region selection, with an injected environment and injected
//! config files: nothing here reads the real `~/.aws` or the process environment.

use super::*;
use crate::settings::StaticCredentials;
use aws_credential_types::provider::ProvideCredentials;
use aws_runtime::env_config::file::EnvConfigFileKind;
use aws_types::os_shim_internal::{Env, Fs};
use std::time::Duration;

const CONFIG: &str = "\
[default]
region = ap-south-1
aws_access_key_id = AKIADEFAULTPROFILE
aws_secret_access_key = default-secret

[profile prod-ro]
region = eu-west-3
aws_access_key_id = AKIAPRODRO
aws_secret_access_key = prod-secret
aws_session_token = prod-session

[profile no-region]
aws_access_key_id = AKIANOREGION
aws_secret_access_key = noregion-secret
";

fn files() -> EnvConfigFiles {
    EnvConfigFiles::builder()
        .include_default_config_file(false)
        .include_default_credentials_file(false)
        .with_contents(EnvConfigFileKind::Config, CONFIG)
        .build()
}

fn keys_only(id: &str) -> AwsSettings {
    AwsSettings {
        credentials: Some(StaticCredentials::new(id, "static-secret", None)),
        ..Default::default()
    }
}

async fn access_key(config: &SdkConfig) -> String {
    config
        .credentials_provider()
        .expect("a credentials provider")
        .provide_credentials()
        .await
        .expect("credentials resolve")
        .access_key_id()
        .to_string()
}

/// Load a config whose environment and files are the injected ones.
async fn load(settings: &AwsSettings, env: &[(&str, &str)]) -> SdkConfig {
    configure_loader(settings, Some(&files()))
        .await
        .unwrap()
        .env(Env::from_slice(env))
        .fs(Fs::from_slice(&[]))
        .load()
        .await
}

#[tokio::test]
async fn static_keys_beat_the_environment() {
    let env = [
        ("AWS_ACCESS_KEY_ID", "AKIAFROMENV"),
        ("AWS_SECRET_ACCESS_KEY", "env-secret"),
    ];
    let config = load(&keys_only("AKIASTATIC"), &env).await;
    assert_eq!(access_key(&config).await, "AKIASTATIC");
}

#[tokio::test]
async fn static_keys_carry_the_session_token() {
    let settings = AwsSettings {
        credentials: Some(StaticCredentials::new(
            "AKIA1",
            "secret",
            Some("tok".into()),
        )),
        ..Default::default()
    };
    let config = load(&settings, &[]).await;
    let credentials = config
        .credentials_provider()
        .unwrap()
        .provide_credentials()
        .await
        .unwrap();
    assert_eq!(credentials.session_token(), Some("tok"));
}

#[tokio::test]
async fn the_default_chain_reads_the_environment() {
    let env = [
        ("AWS_ACCESS_KEY_ID", "AKIAFROMENV"),
        ("AWS_SECRET_ACCESS_KEY", "env-secret"),
        ("AWS_REGION", "us-east-2"),
    ];
    let config = load(&AwsSettings::default(), &env).await;
    assert_eq!(access_key(&config).await, "AKIAFROMENV");
    assert_eq!(config.region().map(Region::as_ref), Some("us-east-2"));
}

#[tokio::test]
async fn an_explicit_profile_beats_environment_credentials() {
    let env = [
        ("AWS_ACCESS_KEY_ID", "AKIAFROMENV"),
        ("AWS_SECRET_ACCESS_KEY", "env-secret"),
    ];
    let settings = AwsSettings {
        profile: Some("prod-ro".into()),
        ..Default::default()
    };
    let config = load(&settings, &env).await;
    assert_eq!(access_key(&config).await, "AKIAPRODRO");
}

#[tokio::test]
async fn a_profile_without_keys_in_the_environment_still_resolves() {
    let settings = AwsSettings {
        profile: Some("no-region".into()),
        ..Default::default()
    };
    let config = load(&settings, &[]).await;
    assert_eq!(access_key(&config).await, "AKIANOREGION");
}

#[tokio::test]
async fn an_unknown_profile_is_an_error_not_a_silent_fallback() {
    let settings = AwsSettings {
        profile: Some("does-not-exist".into()),
        ..Default::default()
    };
    let env = [
        ("AWS_ACCESS_KEY_ID", "AKIAFROMENV"),
        ("AWS_SECRET_ACCESS_KEY", "env-secret"),
    ];
    let config = load(&settings, &env).await;
    let result = config
        .credentials_provider()
        .unwrap()
        .provide_credentials()
        .await;
    assert!(result.is_err(), "must not fall back to the environment");
}

#[tokio::test]
async fn region_precedence_is_url_then_profile_then_environment() {
    let env = [("AWS_REGION", "us-east-2")];
    // ?region= wins over everything
    let explicit = AwsSettings {
        region: Some("eu-central-1".into()),
        profile: Some("prod-ro".into()),
        ..Default::default()
    };
    assert_eq!(
        load(&explicit, &env).await.region().map(Region::as_ref),
        Some("eu-central-1")
    );
    // an explicit profile's region wins over the environment
    let profile = AwsSettings {
        profile: Some("prod-ro".into()),
        ..Default::default()
    };
    assert_eq!(
        load(&profile, &env).await.region().map(Region::as_ref),
        Some("eu-west-3")
    );
    // a profile without a region falls back to the environment
    let no_region = AwsSettings {
        profile: Some("no-region".into()),
        ..Default::default()
    };
    assert_eq!(
        load(&no_region, &[("AWS_REGION", "us-east-2")])
            .await
            .region()
            .map(Region::as_ref),
        Some("us-east-2")
    );
}

#[tokio::test]
async fn anonymous_means_no_credentials_provider() {
    let settings = AwsSettings {
        anonymous: true,
        ..Default::default()
    };
    let config = load(&settings, &[]).await;
    assert!(config.credentials_provider().is_none());
}

#[tokio::test]
async fn endpoint_timeouts_and_retries_are_applied() {
    let settings = AwsSettings {
        endpoint_url: Some("http://127.0.0.1:9000".into()),
        connect_timeout: Duration::from_secs(4),
        operation_timeout: Some(Duration::from_secs(20)),
        max_attempts: 5,
        ..Default::default()
    };
    let config = load(&settings, &[]).await;
    assert_eq!(config.endpoint_url(), Some("http://127.0.0.1:9000"));
    let timeouts = config.timeout_config().unwrap();
    assert_eq!(timeouts.connect_timeout(), Some(Duration::from_secs(4)));
    assert_eq!(timeouts.operation_timeout(), Some(Duration::from_secs(20)));
    assert_eq!(config.retry_config().unwrap().max_attempts(), 5);
    assert!(
        config.http_client().is_some(),
        "the ring client is injected"
    );
}

#[test]
fn the_credential_choice_follows_the_source() {
    let http = http::ring_client(None).unwrap();
    let choice = |s: &AwsSettings| credential_provider_for(s, &http, Some(&files()));
    assert!(matches!(
        choice(&AwsSettings::default()),
        CredentialsChoice::Chain
    ));
    assert!(matches!(
        choice(&AwsSettings {
            anonymous: true,
            ..Default::default()
        }),
        CredentialsChoice::Anonymous
    ));
    assert!(matches!(
        choice(&keys_only("AKIA1")),
        CredentialsChoice::Provider(_)
    ));
    assert!(matches!(
        choice(&AwsSettings {
            profile: Some("prod-ro".into()),
            ..Default::default()
        }),
        CredentialsChoice::Provider(_)
    ));
}

#[tokio::test]
async fn profile_files_can_come_from_a_temp_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials");
    std::fs::write(
        &path,
        "[tmp]\naws_access_key_id = AKIATEMPFILE\naws_secret_access_key = s\n",
    )
    .unwrap();
    let files = EnvConfigFiles::builder()
        .include_default_config_file(false)
        .include_default_credentials_file(false)
        .with_file(EnvConfigFileKind::Credentials, path)
        .build();
    let settings = AwsSettings {
        profile: Some("tmp".into()),
        ..Default::default()
    };
    let config = configure_loader(&settings, Some(&files))
        .await
        .unwrap()
        .env(Env::from_slice(&[]))
        .fs(Fs::real())
        .load()
        .await;
    assert_eq!(access_key(&config).await, "AKIATEMPFILE");
}

#[tokio::test]
async fn assume_role_wraps_the_base_credentials() {
    let settings = AwsSettings {
        credentials: Some(StaticCredentials::new("AKIABASE", "secret", None)),
        role_arn: Some("arn:aws:iam::123456789012:role/reader".into()),
        external_id: Some("ext".into()),
        region: Some("eu-west-1".into()),
        ..Default::default()
    };
    let base = load(&settings, &[]).await;
    // Building the provider does not call STS; resolving credentials would
    let wrapped = assume_role(&settings, base).await.unwrap();
    assert!(wrapped.credentials_provider().is_some());
}

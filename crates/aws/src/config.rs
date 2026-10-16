//! Build the `SdkConfig` of a connection: region, endpoint, timeouts, retries,
//! the ring HTTP client and the credential provider its settings select.

use crate::settings::{AwsSettings, CredentialSource};
use crate::{AwsError, http};
use aws_config::meta::region::ProvideRegion;
use aws_config::profile::{ProfileFileCredentialsProvider, ProfileFileRegionProvider};
use aws_config::provider_config::ProviderConfig;
use aws_config::retry::RetryConfig;
use aws_config::sts::AssumeRoleProvider;
use aws_config::timeout::TimeoutConfig;
use aws_config::{BehaviorVersion, ConfigLoader};
use aws_credential_types::Credentials;
use aws_credential_types::provider::SharedCredentialsProvider;
use aws_runtime::env_config::file::EnvConfigFiles;
use aws_types::SdkConfig;
use aws_types::region::Region;
use aws_types::sdk_config::SharedHttpClient;

/// How the loader gets its credentials.
pub(crate) enum CredentialsChoice {
    /// The SDK default chain (environment → shared config → … → IMDS).
    Chain,
    /// No credentials at all: the service client sends unsigned requests.
    Anonymous,
    /// An explicit provider (static keys or a named profile).
    Provider(SharedCredentialsProvider),
}

/// The credential provider [`AwsSettings::credential_source`] selects.
/// `files` replaces `~/.aws/{config,credentials}` (tests, a future broker build).
pub(crate) fn credential_provider_for(
    settings: &AwsSettings,
    http: &SharedHttpClient,
    files: Option<&EnvConfigFiles>,
) -> CredentialsChoice {
    match settings.credential_source() {
        CredentialSource::Anonymous => CredentialsChoice::Anonymous,
        CredentialSource::DefaultChain => CredentialsChoice::Chain,
        CredentialSource::Static => {
            let keys = settings
                .credentials
                .as_ref()
                .expect("a static source has credentials");
            CredentialsChoice::Provider(SharedCredentialsProvider::new(Credentials::new(
                keys.access_key_id.clone(),
                keys.secret_access_key.clone(),
                keys.session_token.clone(),
                None,
                "dbcrust-connection",
            )))
        }
        CredentialSource::Profile(name) => {
            let mut conf = ProviderConfig::default().with_http_client(http.clone());
            if let Some(region) = &settings.region {
                conf = conf.with_region(Some(Region::new(region.clone())));
            }
            let mut builder = ProfileFileCredentialsProvider::builder()
                .profile_name(name)
                .configure(&conf);
            if let Some(files) = files {
                builder = builder.profile_files(files.clone());
            }
            CredentialsChoice::Provider(SharedCredentialsProvider::new(builder.build()))
        }
    }
}

/// The explicit region: `?region=`, else the region of an explicit
/// `?profile=`. `None` leaves the SDK default chain (environment, default
/// profile, IMDS) to the loader, which hands it the HTTP client; building that
/// chain here would not (its IMDS leg panics without one).
async fn explicit_region(settings: &AwsSettings, files: Option<&EnvConfigFiles>) -> Option<Region> {
    if let Some(region) = &settings.region {
        return Some(Region::new(region.clone()));
    }
    let profile = settings.profile.as_ref()?;
    let mut builder = ProfileFileRegionProvider::builder().profile_name(profile.clone());
    if let Some(files) = files {
        builder = builder.profile_files(files.clone());
    }
    builder.build().region().await
}

/// A loader with everything but the process environment and files decided.
pub(crate) async fn configure_loader(
    settings: &AwsSettings,
    files: Option<&EnvConfigFiles>,
) -> Result<ConfigLoader, AwsError> {
    settings.validate()?;
    let http = http::ring_client(settings.ca_cert.as_deref())?;
    let mut timeouts = TimeoutConfig::builder();
    timeouts
        .set_connect_timeout(Some(settings.connect_timeout))
        .set_operation_timeout(settings.operation_timeout);
    let timeouts = timeouts.build();
    let mut loader = aws_config::defaults(BehaviorVersion::latest())
        .http_client(http.clone())
        .timeout_config(timeouts)
        .retry_config(RetryConfig::standard().with_max_attempts(settings.max_attempts));

    if let Some(region) = explicit_region(settings, files).await {
        loader = loader.region(region);
    }
    if let Some(endpoint) = &settings.endpoint_url {
        loader = loader.endpoint_url(endpoint.clone());
    }
    if let Some(files) = files {
        loader = loader.profile_files(files.clone());
    }
    if let Some(profile) = &settings.profile {
        loader = loader.profile_name(profile.clone());
    }
    loader = match credential_provider_for(settings, &http, files) {
        CredentialsChoice::Chain => loader,
        CredentialsChoice::Anonymous => loader.no_credentials(),
        CredentialsChoice::Provider(provider) => loader.credentials_provider(provider),
    };
    Ok(loader)
}

/// Load the SDK configuration of a connection. Nothing is requested from the
/// network here except what a profile with SSO / role assumption needs when its
/// credentials are first used.
pub async fn load_sdk_config(settings: &AwsSettings) -> Result<SdkConfig, AwsError> {
    let config = configure_loader(settings, None).await?.load().await;
    assume_role(settings, config).await
}

/// `role_arn`: STS `AssumeRole` on top of whatever credentials the config has.
async fn assume_role(settings: &AwsSettings, config: SdkConfig) -> Result<SdkConfig, AwsError> {
    let Some(role) = &settings.role_arn else {
        return Ok(config);
    };
    let mut builder = AssumeRoleProvider::builder(role.clone())
        .session_name(
            settings
                .session_name
                .clone()
                .unwrap_or_else(|| "dbcrust".to_string()),
        )
        .configure(&config);
    if let Some(external_id) = &settings.external_id {
        builder = builder.external_id(external_id.clone());
    }
    let provider = builder.build().await;
    Ok(config
        .into_builder()
        .credentials_provider(SharedCredentialsProvider::new(provider))
        .build())
}

#[cfg(test)]
mod tests;

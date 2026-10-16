use super::*;
use rstest::rstest;

fn info(
    host: Option<&str>,
    port: Option<u16>,
    tls: bool,
    user: Option<&str>,
    pass: Option<&str>,
    options: &[(&str, &str)],
) -> ConnectionInfo {
    ConnectionInfo {
        host: host.map(str::to_string),
        port,
        use_tls: tls,
        username: user.map(str::to_string),
        password: pass.map(str::to_string),
        options: options
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    }
}

fn settings(options: &[(&str, &str)]) -> Result<AwsSettings, AwsError> {
    AwsSettings::from_connection_info(&info(None, None, true, None, None, options))
}

#[test]
fn an_empty_connection_uses_the_default_chain_on_aws() {
    let s = settings(&[]).unwrap();
    assert_eq!(s.credential_source(), CredentialSource::DefaultChain);
    assert_eq!(s.endpoint_url, None);
    assert!(!s.force_path_style);
    assert_eq!(s.region, None);
    assert_eq!(s.max_attempts, DEFAULT_MAX_ATTEMPTS);
    assert_eq!(s.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
}

#[rstest]
#[case(Some("localhost"), Some(9000), false, Some("http://localhost:9000"))]
#[case(Some("minio.internal"), None, true, Some("https://minio.internal"))]
#[case(Some("::1"), Some(9000), false, Some("http://[::1]:9000"))]
#[case(Some("[::1]"), Some(9000), true, Some("https://[::1]:9000"))]
#[case(
    Some("acc.r2.cloudflarestorage.com"),
    None,
    true,
    Some("https://acc.r2.cloudflarestorage.com")
)]
#[case(None, Some(9000), true, None)]
fn the_endpoint_comes_from_host_port_and_tls(
    #[case] host: Option<&str>,
    #[case] port: Option<u16>,
    #[case] tls: bool,
    #[case] expected: Option<&str>,
) {
    let s = AwsSettings::from_connection_info(&info(host, port, tls, None, None, &[])).unwrap();
    assert_eq!(s.endpoint_url.as_deref(), expected);
}

#[rstest]
#[case("bad host")]
#[case("host/path")]
#[case("user@host")]
fn malformed_endpoint_hosts_are_refused(#[case] host: &str) {
    let result = AwsSettings::from_connection_info(&info(Some(host), None, true, None, None, &[]));
    assert!(matches!(result, Err(AwsError::InvalidOption(_))), "{host}");
}

#[rstest]
#[case(Some("localhost"), &[], true)]
#[case(Some("localhost"), &[("path_style", "false")], false)]
#[case(Some("s3.eu-west-1.amazonaws.com"), &[], false)]
#[case(Some("s3.eu-west-1.amazonaws.com"), &[("force_path_style", "true")], true)]
#[case(None, &[], false)]
#[case(None, &[("path_style", "yes")], true)]
fn path_style_defaults_to_on_for_custom_endpoints_only(
    #[case] host: Option<&str>,
    #[case] options: &[(&str, &str)],
    #[case] expected: bool,
) {
    let s = AwsSettings::from_connection_info(&info(host, Some(9000), false, None, None, options))
        .unwrap();
    assert_eq!(s.force_path_style, expected);
}

#[test]
fn a_bad_boolean_is_refused() {
    let error = settings(&[("path_style", "maybe")]).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("path_style must be true or false")
    );
}

#[rstest]
#[case("eu-west-1", true)]
#[case("us-gov-east-1", true)]
#[case("auto", true)]
#[case("EU-WEST-1", false)]
#[case("eu west", false)]
#[case("eu-west-1.evil.example", false)]
#[case("", false)]
fn regions_are_validated(#[case] region: &str, #[case] ok: bool) {
    let result = settings(&[("region", region)]);
    // an empty value is "not given"
    if region.is_empty() {
        assert_eq!(result.unwrap().region, None);
    } else {
        assert_eq!(result.is_ok(), ok, "{region}");
    }
}

#[test]
fn credential_source_priority() {
    let keys = |extra: &[(&str, &str)]| {
        AwsSettings::from_connection_info(&info(
            None,
            None,
            true,
            Some("AKIAEXAMPLE"),
            Some("s3cr3t"),
            extra,
        ))
    };
    assert_eq!(
        keys(&[]).unwrap().credential_source(),
        CredentialSource::Static
    );
    assert_eq!(
        settings(&[("profile", "prod-ro")])
            .unwrap()
            .credential_source(),
        CredentialSource::Profile("prod-ro".to_string())
    );
    assert_eq!(
        settings(&[("anonymous", "true")])
            .unwrap()
            .credential_source(),
        CredentialSource::Anonymous
    );
    // contradictions are errors, not silent priorities
    assert!(keys(&[("profile", "x")]).is_err());
    assert!(keys(&[("anonymous", "true")]).is_err());
    assert!(settings(&[("anonymous", "true"), ("profile", "x")]).is_err());
    assert!(settings(&[("anonymous", "true"), ("role_arn", "arn:aws:iam::1:role/r")]).is_err());
}

#[test]
fn half_a_key_pair_is_refused_with_a_hint() {
    let only_id =
        AwsSettings::from_connection_info(&info(None, None, true, Some("AKIAEXAMPLE"), None, &[]))
            .unwrap_err();
    assert!(only_id.to_string().contains("needs its secret access key"));
    let only_secret =
        AwsSettings::from_connection_info(&info(None, None, true, None, Some("s3cr3t"), &[]))
            .unwrap_err();
    assert!(only_secret.to_string().contains("needs its access key id"));
    let token_alone = settings(&[("session_token", "tok")]).unwrap_err();
    assert!(token_alone.to_string().contains("session_token goes with"));
}

#[test]
fn static_credentials_never_show_in_debug_or_errors() {
    let mut conn = info(
        None,
        None,
        true,
        Some("AKIAEXAMPLE"),
        Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYSECRETKEY"),
        &[("session_token", "FwoGZXIvYXdzSESSIONTOKEN")],
    );
    let s = AwsSettings::from_connection_info(&conn).unwrap();
    let debug = format!("{s:?}");
    assert!(debug.contains("AKIAEXAMPLE"));
    assert!(!debug.contains("SECRETKEY"), "{debug}");
    assert!(!debug.contains("SESSIONTOKEN"), "{debug}");
    assert_eq!(s.secrets().len(), 2);

    conn.options.insert("region".into(), "NOT A REGION".into());
    let error = AwsSettings::from_connection_info(&conn)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("SECRETKEY") && !error.contains("SESSIONTOKEN"));
}

#[test]
fn role_options_are_validated_together() {
    let ok = settings(&[
        ("role_arn", "arn:aws:iam::123456789012:role/reader"),
        ("external_id", "ext-1"),
        ("session_name", "dbcrust"),
    ])
    .unwrap();
    assert_eq!(
        ok.role_arn.as_deref(),
        Some("arn:aws:iam::123456789012:role/reader")
    );
    assert!(settings(&[("role_arn", "not-an-arn")]).is_err());
    assert!(settings(&[("external_id", "ext-1")]).is_err());
}

#[rstest]
#[case("connect_timeout", "0")]
#[case("connect_timeout", "601")]
#[case("connect_timeout", "ten")]
#[case("max_attempts", "0")]
#[case("max_attempts", "11")]
#[case("operation_timeout", "-1")]
fn limits_are_bounded(#[case] key: &str, #[case] value: &str) {
    assert!(settings(&[(key, value)]).is_err(), "{key}={value}");
}

#[test]
fn limits_are_read() {
    let s = settings(&[
        ("connect_timeout", "3"),
        ("operation_timeout", "45"),
        ("max_attempts", "5"),
        ("ca_cert", "/etc/ssl/minio-ca.pem"),
    ])
    .unwrap();
    assert_eq!(s.connect_timeout, Duration::from_secs(3));
    assert_eq!(s.operation_timeout, Some(Duration::from_secs(45)));
    assert_eq!(s.max_attempts, 5);
    assert_eq!(s.ca_cert, Some(PathBuf::from("/etc/ssl/minio-ca.pem")));
}

#[test]
fn option_names_are_case_insensitive() {
    let s = settings(&[("Region", "eu-west-1"), ("PROFILE", "dev")]).unwrap();
    assert_eq!(s.region.as_deref(), Some("eu-west-1"));
    assert_eq!(s.profile.as_deref(), Some("dev"));
}

use super::*;

const PLACEHOLDER: &str = "wash_ph_7Qm2xYk9Lr4Tz8Vb1Nc6";
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn credentials(token: Option<&str>) -> Arc<LeasedAwsCredentials> {
    let source = Arc::new(LeasedAwsCredentials::default());
    source.renew(
        AwsCredentials {
            access_key_id: "ASIAREAL".into(),
            secret_access_key: Zeroizing::new("realsecret".into()),
            session_token: token.map(|t| Zeroizing::new(t.to_string())),
            expires_at: None,
        },
        std::time::Duration::from_secs(60),
    );
    source
}

fn secret(source: Arc<LeasedAwsCredentials>) -> SigV4Secret {
    SigV4Secret::new(PLACEHOLDER, source)
        .unwrap()
        .for_destinations(["bucket.s3.us-east-1.amazonaws.com".parse().unwrap()])
        .for_services(["s3"])
        .unwrap()
}

fn request(uri: &str, access_key: &str, sha: Option<&str>) -> hyper::Request<()> {
    let mut builder = hyper::Request::put(uri).header(
        http::header::AUTHORIZATION,
        format!(
            "AWS4-HMAC-SHA256 Credential={access_key}/20200101/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=deadbeef"
        ),
    );
    builder = builder.header("x-amz-date", "20200101T000000Z");
    if let Some(sha) = sha {
        builder = builder.header("x-amz-content-sha256", sha);
    }
    builder.body(()).unwrap()
}

const URI: &str = "https://bucket.s3.us-east-1.amazonaws.com/reports/q3.csv";

fn at() -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_763_337_600)
}

#[test]
fn the_signing_key_and_signature_follow_the_published_example() {
    // AWS's get-vanilla example from the SigV4 test suite.
    let canonical = "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\n\
                     host;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let hashed = hex(&Sha256::digest(canonical.as_bytes()));
    assert_eq!(
        hashed,
        "bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63"
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n{hashed}"
    );
    assert_eq!(
        signature(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "service",
            &string_to_sign
        ),
        "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
    );
}

#[test]
fn the_canonical_request_matches_the_published_example() {
    let request = hyper::Request::get("https://example.amazonaws.com/")
        .header("x-amz-date", "20150830T123600Z")
        .body(())
        .unwrap();
    let canonical = canonical_request(
        &request,
        &["host".into(), "x-amz-date".into()],
        EMPTY_SHA256,
    )
    .unwrap();
    assert_eq!(
        hex(&Sha256::digest(canonical.as_bytes())),
        "bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63"
    );
}

#[test]
fn a_placeholder_key_is_re_signed_with_the_hosts_credentials() {
    let secrets = [secret(credentials(Some("session-token")))];
    let mut req = request(URI, PLACEHOLDER, Some(EMPTY_SHA256));
    let signed = resign(&mut req, &secrets, at()).unwrap().unwrap();
    assert_eq!(&*signed.placeholder, PLACEHOLDER);
    let authorization = req.headers()[http::header::AUTHORIZATION].to_str().unwrap();
    assert!(
        authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=ASIAREAL/20251117/us-east-1/s3/aws4_request, "
        ),
        "{authorization}"
    );
    assert!(
        authorization
            .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token,"),
        "{authorization}"
    );
    assert!(!authorization.contains("deadbeef") && !authorization.contains(PLACEHOLDER));
    assert_eq!(req.headers()["x-amz-security-token"], "session-token");
    assert_eq!(req.headers()["x-amz-date"], "20251117T000000Z");
    assert!(req.extensions().get::<CarriesSecret>().is_some());
}

#[test]
fn a_request_with_no_placeholder_is_left_alone() {
    let secrets = [secret(credentials(None))];
    let mut req = request(URI, "AKIAGUESTOWN", Some(EMPTY_SHA256));
    assert!(matches!(resign(&mut req, &secrets, at()), Ok(None)));
    assert!(
        req.headers()[http::header::AUTHORIZATION]
            .to_str()
            .unwrap()
            .contains("AKIAGUESTOWN")
    );
}

#[test]
fn the_body_is_never_hashed_so_its_hash_must_be_sent() {
    let secrets = [secret(credentials(None))];
    for sha in [
        None,
        Some("STREAMING-AWS4-HMAC-SHA256-PAYLOAD"),
        Some("STREAMING-UNSIGNED-PAYLOAD-TRAILER"),
        Some("not-a-hash"),
    ] {
        let mut req = request(URI, PLACEHOLDER, sha);
        let refused = resign(&mut req, &secrets, at()).unwrap_err();
        assert_eq!(refused.error, CredentialError::Misplaced, "{sha:?}");
    }
    let mut unsigned = request(URI, PLACEHOLDER, Some("UNSIGNED-PAYLOAD"));
    assert!(resign(&mut unsigned, &secrets, at()).is_ok());

    let mut chunked = request(URI, PLACEHOLDER, Some(EMPTY_SHA256));
    chunked.headers_mut().insert(
        http::header::CONTENT_ENCODING,
        HeaderValue::from_static("aws-chunked"),
    );
    assert!(resign(&mut chunked, &secrets, at()).is_err());
}

#[test]
fn presigned_urls_are_never_minted() {
    let secrets = [secret(credentials(None))];
    let uri = format!(
        "{URI}?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={PLACEHOLDER}%2F20200101%2Fus-east-1%2Fs3%2Faws4_request"
    );
    let mut req = hyper::Request::get(uri).body(()).unwrap();
    assert_eq!(
        resign(&mut req, &secrets, at()).unwrap_err().error,
        CredentialError::Misplaced
    );
}

#[test]
fn the_destination_service_and_scheme_are_checked() {
    let secrets = [secret(credentials(None))];
    for (uri, error) in [
        ("https://evil.example.com/x", CredentialError::Destination),
        (
            "https://bucket.s3.us-east-1.amazonaws.com:8443/x",
            CredentialError::Destination,
        ),
        (
            "http://bucket.s3.us-east-1.amazonaws.com/x",
            CredentialError::NotTls,
        ),
    ] {
        let mut req = request(uri, PLACEHOLDER, Some(EMPTY_SHA256));
        assert_eq!(
            resign(&mut req, &secrets, at()).unwrap_err().error,
            error,
            "{uri}"
        );
    }
    let mut other = request(URI, "wash_ph_SomeoneElsesKey00000000", Some(EMPTY_SHA256));
    assert_eq!(
        resign(&mut other, &secrets, at()).unwrap_err().error,
        CredentialError::Unknown
    );
}

#[test]
fn revoked_credentials_refuse_the_request() {
    let source = credentials(None);
    let secrets = [secret(Arc::clone(&source))];
    source.revoke();
    let mut req = request(URI, PLACEHOLDER, Some(EMPTY_SHA256));
    assert_eq!(
        resign(&mut req, &secrets, at()).unwrap_err().error,
        CredentialError::Unavailable
    );
}

#[test]
fn only_s3_family_services_are_signed_and_one_must_be_named() {
    assert!(
        SigV4Secret::new(PLACEHOLDER, credentials(None))
            .unwrap()
            .for_services(["sqs"])
            .is_err()
    );
    for service in S3_SERVICES {
        assert!(
            SigV4Secret::new(PLACEHOLDER, credentials(None))
                .unwrap()
                .for_services([service])
                .is_ok()
        );
    }
    let unscoped = SigV4Secret::new(PLACEHOLDER, credentials(None))
        .unwrap()
        .for_destinations(["bucket.s3.us-east-1.amazonaws.com".parse().unwrap()]);
    let mut req = request(URI, PLACEHOLDER, Some(EMPTY_SHA256));
    assert_eq!(
        resign(&mut req, &[unscoped], at()).unwrap_err().error,
        CredentialError::Destination
    );
}

#[test]
fn s3_paths_are_signed_as_sent() {
    let request = hyper::Request::get("https://bucket.s3.amazonaws.com/a%20b//c/../d")
        .body(())
        .unwrap();
    let canonical = canonical_request(&request, &["host".into()], EMPTY_SHA256).unwrap();
    assert!(canonical.contains("\n/a%20b//c/../d\n"), "{canonical}");
}

#[test]
fn query_parameters_are_sorted_and_encoded() {
    let request =
        hyper::Request::get("https://bucket.s3.amazonaws.com/?prefix=a%2Fb&list-type=2&x")
            .body(())
            .unwrap();
    let canonical = canonical_request(&request, &["host".into()], EMPTY_SHA256).unwrap();
    assert!(
        canonical.contains("\nlist-type=2&prefix=a%2Fb&x=\n"),
        "{canonical}"
    );
    let binary = hyper::Request::get("https://bucket.s3.amazonaws.com/?k=%FF%FE")
        .body(())
        .unwrap();
    let canonical = canonical_request(&binary, &["host".into()], EMPTY_SHA256).unwrap();
    assert!(canonical.contains("\nk=%FF%FE\n"), "{canonical}");
}

/// The re-signed request carries the signature AWS's own signer computes for
/// the same request, session token and query included.
#[test]
fn the_signature_matches_an_independent_signer() {
    use aws_sigv4::http_request::{
        PayloadChecksumKind, PercentEncodingMode, SessionTokenMode, SignableBody, SignableRequest,
        SigningSettings, UriPathNormalizationMode, sign,
    };

    let uri = "https://bucket.s3.us-east-1.amazonaws.com/reports/q%203.csv?versionId=a%2Fb&list-type=2&prefix=%E2%9C%93&tag=a+b";
    let secrets = [secret(credentials(Some("session/token+value")))];
    let mut req = hyper::Request::put(uri)
        .header(
            http::header::AUTHORIZATION,
            format!(
                "AWS4-HMAC-SHA256 Credential={PLACEHOLDER}/20200101/us-east-1/s3/aws4_request, \
                 SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-meta-note;x-amz-meta-tab, Signature=deadbeef"
            ),
        )
        .header("x-amz-date", "20200101T000000Z")
        .header("x-amz-content-sha256", EMPTY_SHA256)
        .header("x-amz-meta-note", "  two   spaces ")
        .header("x-amz-meta-tab", "a\t b")
        .body(())
        .unwrap();
    resign(&mut req, &secrets, at()).unwrap();
    let authorization = req.headers()[http::header::AUTHORIZATION].to_str().unwrap();
    let ours = authorization.rsplit_once("Signature=").unwrap().1;

    let identity = aws_credential_types::Credentials::new(
        "ASIAREAL",
        "realsecret",
        Some("session/token+value".into()),
        None,
        "test",
    )
    .into();
    let mut settings = SigningSettings::default();
    settings.percent_encoding_mode = PercentEncodingMode::Single;
    settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
    settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    settings.session_token_mode = SessionTokenMode::Include;
    let params = aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region("us-east-1")
        .name("s3")
        .time(at())
        .settings(settings)
        .build()
        .unwrap()
        .into();
    let signable = SignableRequest::new(
        "PUT",
        uri,
        [
            ("x-amz-meta-note", "  two   spaces "),
            ("x-amz-meta-tab", "a\t b"),
        ]
        .into_iter(),
        SignableBody::Precomputed(EMPTY_SHA256.into()),
    )
    .unwrap();
    let (_, theirs) = sign(signable, &params).unwrap().into_parts();
    assert_eq!(ours, theirs);
}

const META_PLACEHOLDER: &str = "wash_ph_Zx81Qa4Ws7Ed2Rf5Tg9Yh3Uj";

fn meta_secret() -> crate::host::egress_credential::EgressSecret {
    crate::host::egress_credential::EgressSecret::new(
        META_PLACEHOLDER,
        Arc::new(crate::host::egress_credential::tests::Fixed(Some(
            b"real-meta-0123456789",
        ))),
        crate::host::egress_credential::SecretForm::Header("x-amz-meta-key".parse().unwrap()),
    )
    .unwrap()
    .for_destinations(["bucket.s3.us-east-1.amazonaws.com".parse().unwrap()])
    .unwrap()
}

fn signed_with_meta(uri: &str, meta: &str) -> hyper::Request<()> {
    hyper::Request::put(uri)
        .header(
            http::header::AUTHORIZATION,
            format!(
                "AWS4-HMAC-SHA256 Credential={PLACEHOLDER}/20200101/us-east-1/s3/aws4_request, \
                 SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-meta-key, Signature=deadbeef"
            ),
        )
        .header("x-amz-date", "20200101T000000Z")
        .header("x-amz-content-sha256", EMPTY_SHA256)
        .header("x-amz-meta-key", meta)
        .body(())
        .unwrap()
}

#[test]
fn a_substituted_signed_header_is_signed_with_its_real_value() {
    let signers = [secret(credentials(None))];
    let mut req = signed_with_meta(URI, META_PLACEHOLDER);
    let done = substitute_and_sign(&mut req, &[meta_secret()], &signers, at()).unwrap();
    assert_eq!(req.headers()["x-amz-meta-key"], "real-meta-0123456789");
    assert_eq!(done.used.len(), 2);

    let mut expected = signed_with_meta(URI, "real-meta-0123456789");
    resign(&mut expected, &signers, at()).unwrap();
    assert_eq!(
        req.headers()[http::header::AUTHORIZATION],
        expected.headers()[http::header::AUTHORIZATION]
    );
}

#[test]
fn a_placeholder_left_in_what_is_signed_is_refused() {
    let signers = [secret(credentials(None))];
    let mut header = signed_with_meta(URI, "wash_ph_NotAnyKnownSecretAtAll1");
    assert_eq!(
        resign(&mut header, &signers, at()).unwrap_err().error,
        CredentialError::Misplaced
    );

    let mut query = request(
        &format!("{URI}?note=wash%5Fph%5FNotAnyKnownSecretAtAll1"),
        PLACEHOLDER,
        Some(EMPTY_SHA256),
    );
    assert_eq!(
        resign(&mut query, &signers, at()).unwrap_err().error,
        CredentialError::Misplaced
    );
}

#[test]
fn the_host_is_signed_as_it_is_sent() {
    let signers = [secret(credentials(None))];
    let mut default_port = request(
        "https://bucket.s3.us-east-1.amazonaws.com:443/reports/q3.csv",
        PLACEHOLDER,
        Some(EMPTY_SHA256),
    );
    resign(&mut default_port, &signers, at()).unwrap();
    assert_eq!(
        default_port.headers()[http::header::HOST],
        "bucket.s3.us-east-1.amazonaws.com"
    );
    let mut plain = request(URI, PLACEHOLDER, Some(EMPTY_SHA256));
    resign(&mut plain, &signers, at()).unwrap();
    assert_eq!(
        default_port.headers()[http::header::AUTHORIZATION],
        plain.headers()[http::header::AUTHORIZATION]
    );

    let mut userinfo = request(
        "https://user@bucket.s3.us-east-1.amazonaws.com/reports/q3.csv",
        PLACEHOLDER,
        Some(EMPTY_SHA256),
    );
    assert_eq!(
        resign(&mut userinfo, &signers, at()).unwrap_err().error,
        CredentialError::Destination
    );
}

#[test]
fn a_sigv4_placeholder_has_the_shape_the_host_looks_for() {
    for short in [
        "wash_ph_",
        "wash_ph_short",
        "AKIAwash_ph_7Qm2xYk9Lr4Tz8Vb1N_c6",
    ] {
        assert!(
            SigV4Secret::new(short, credentials(None)).is_err(),
            "{short}"
        );
    }
    assert!(SigV4Secret::new("AKIAwash_ph_7Qm2xYk9Lr4Tz8Vb1Nc6", credentials(None)).is_ok());
}

#[test]
fn the_guest_token_is_held_back_and_the_host_values_reverted() {
    let signers = [secret(credentials(Some("host-session-token")))];
    let mut req = request(URI, PLACEHOLDER, Some(EMPTY_SHA256));
    req.headers_mut()
        .insert("x-amz-security-token", META_PLACEHOLDER.parse().unwrap());
    let done = substitute_and_sign(&mut req, &[], &signers, at()).unwrap();
    assert_eq!(req.headers()["x-amz-security-token"], "host-session-token");
    assert_eq!(done.signed().len(), 2);

    let mut echoed = http::HeaderMap::new();
    echoed.insert("x-amz-echo", "ASIAREAL host-session-token".parse().unwrap());
    done.revert(&mut echoed);
    assert_eq!(
        echoed["x-amz-echo"].to_str().unwrap(),
        format!("{PLACEHOLDER} {PLACEHOLDER}")
    );
}

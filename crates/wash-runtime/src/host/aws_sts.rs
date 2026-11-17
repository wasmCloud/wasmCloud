//! AWS credentials from a workload's JWT-SVID, through STS
//! `AssumeRoleWithWebIdentity`, so no AWS key exists anywhere.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use http_body_util::{BodyExt as _, Full, Limited};
use tracing::warn;
use zeroize::{Zeroize as _, Zeroizing};

use crate::host::egress_credential::EgressCredentialSource;
use crate::host::egress_sigv4::{AwsCredentials, LeasedAwsCredentials};
use crate::host::http_client::{Alpn, https_connector};

/// The global STS endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://sts.amazonaws.com/";

/// Largest STS response read.
const MAX_RESPONSE: usize = 64 * 1024;

/// How long to wait before asking STS again after a failure.
const RETRY: Duration = Duration::from_secs(5);

/// Longest one exchange, response included, may take.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);

/// Exchanges a web identity token for temporary credentials for one role.
#[derive(Clone)]
pub struct StsWebIdentity {
    endpoint: http::Uri,
    role_arn: String,
    session_name: String,
    duration: Duration,
    tls: Arc<rustls::ClientConfig>,
}

impl StsWebIdentity {
    /// `session_name` appears in CloudTrail; AWS allows 2 to 64 of
    /// letters, digits and `+=,.@_-`.
    pub fn new(
        role_arn: &str,
        session_name: &str,
        tls: Arc<rustls::ClientConfig>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            role_arn.starts_with("arn:") && role_arn.contains(":role/"),
            "{role_arn:?} is not an IAM role ARN"
        );
        anyhow::ensure!(
            (2..=64).contains(&session_name.len())
                && session_name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+=,.@_-".contains(&b)),
            "{session_name:?} is not a valid role session name"
        );
        Ok(Self {
            endpoint: http::Uri::from_static(DEFAULT_ENDPOINT),
            role_arn: role_arn.to_string(),
            session_name: session_name.to_string(),
            duration: Duration::from_secs(3600),
            tls,
        })
    }

    /// A regional or private STS endpoint. Must be https.
    pub fn with_endpoint(mut self, endpoint: &str) -> anyhow::Result<Self> {
        let endpoint: http::Uri = endpoint.parse()?;
        anyhow::ensure!(
            endpoint.scheme() == Some(&http::uri::Scheme::HTTPS),
            "the STS endpoint must be https"
        );
        self.endpoint = endpoint;
        Ok(self)
    }

    #[cfg(test)]
    pub(crate) fn with_test_endpoint(mut self, endpoint: http::Uri) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// How long the credentials asked for last, 15 minutes to 12 hours.
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration.clamp(Duration::from_secs(900), Duration::from_secs(43_200));
        self
    }

    /// Exchange `token` for credentials.
    ///
    /// The copies of the token and of the response this holds are zeroized
    /// when dropped.
    pub async fn assume(&self, token: &[u8]) -> anyhow::Result<AwsCredentials> {
        let token = std::str::from_utf8(token)?;
        let body = form(&[
            ("Action", "AssumeRoleWithWebIdentity"),
            ("Version", "2011-06-15"),
            ("RoleArn", &self.role_arn),
            ("RoleSessionName", &self.session_name),
            ("DurationSeconds", &self.duration.as_secs().to_string()),
            ("WebIdentityToken", token),
        ]);
        let request = http::Request::post(self.endpoint.clone())
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(http::header::ACCEPT, "application/xml")
            .body(Full::new(SecretBuf {
                bytes: body,
                read: 0,
            }))?;
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(https_connector(&self.tls, Alpn::Http1));
        let exchange = async {
            let response = client.request(request).await?;
            let status = response.status();
            let body = Limited::new(response.into_body(), MAX_RESPONSE)
                .collect()
                .await
                .map_err(|e| anyhow::anyhow!("failed to read the STS response: {e}"))?
                .to_bytes();
            anyhow::Ok((status, secret_text(body)))
        };
        let (status, body) = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange)
            .await
            .map_err(|_| anyhow::anyhow!("STS did not answer in time"))??;
        if !status.is_success() {
            let code = element(&body, "Code").unwrap_or_default();
            anyhow::bail!("STS refused the web identity ({status}, {})", code.as_str());
        }
        let field = |name| {
            element(&body, name).ok_or_else(|| anyhow::anyhow!("the STS response has no {name}"))
        };
        let expiration = field("Expiration")?;
        let expires_at: SystemTime = chrono::DateTime::parse_from_rfc3339(&expiration)?
            .with_timezone(&chrono::Utc)
            .into();
        anyhow::ensure!(
            expires_at > SystemTime::now(),
            "STS returned credentials that have already expired"
        );
        Ok(AwsCredentials {
            access_key_id: field("AccessKeyId")?.to_string(),
            secret_access_key: field("SecretAccessKey")?,
            session_token: Some(field("SessionToken")?),
            expires_at: Some(expires_at),
        })
    }
}

/// A request body that zeroizes itself when dropped.
struct SecretBuf {
    bytes: Zeroizing<String>,
    read: usize,
}

impl bytes::Buf for SecretBuf {
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.read)
    }

    fn chunk(&self) -> &[u8] {
        self.bytes.as_bytes().get(self.read..).unwrap_or_default()
    }

    fn advance(&mut self, count: usize) {
        self.read = self.read.saturating_add(count).min(self.bytes.len());
    }
}

/// `body` as text, zeroizing `body` itself when nothing else shares it.
fn secret_text(body: bytes::Bytes) -> Zeroizing<String> {
    let text = Zeroizing::new(String::from_utf8_lossy(&body).into_owned());
    if let Ok(mut unique) = body.try_into_mut() {
        unique.as_mut().zeroize();
    }
    text
}

/// Keep `into` holding credentials for `sts`'s role, exchanging the token
/// `identity` holds, until the returned task is aborted or `identity` is
/// revoked.
///
/// Credentials are exchanged again half way through their life. They are
/// leased only until they expire, so a failing exchange leaves `into` empty
/// once the last credentials run out rather than serving stale ones. Once
/// `identity` is revoked, even mid-exchange, `into` is closed for good and
/// the task ends, so give each refresh its own `into`.
pub fn spawn_web_identity_refresh(
    sts: StsWebIdentity,
    identity: Arc<dyn EgressCredentialSource>,
    into: Arc<LeasedAwsCredentials>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let revoked = identity.revocation().unwrap_or_default();
        while !revoked.is_cancelled() {
            let wait = match identity.current() {
                Err(_) => RETRY,
                Ok(token) => {
                    let assumed = tokio::select! {
                        () = revoked.cancelled() => break,
                        assumed = sts.assume(&token) => assumed,
                    };
                    match assumed {
                        Ok(_) if revoked.is_cancelled() => break,
                        Ok(credentials) => {
                            let left = credentials
                                .expires_at
                                .and_then(|at| at.duration_since(SystemTime::now()).ok())
                                .unwrap_or_default();
                            into.renew(credentials, left);
                            (left / 2).max(RETRY)
                        }
                        Err(err) => {
                            warn!(role_arn = sts.role_arn, %err, "failed to exchange a web identity for AWS credentials");
                            RETRY
                        }
                    }
                }
            };
            tokio::select! {
                () = revoked.cancelled() => break,
                () = tokio::time::sleep(wait) => {}
            }
        }
        into.close();
    })
}

/// `pairs` form-encoded straight into a zeroizing buffer.
pub(crate) fn form(pairs: &[(&str, &str)]) -> Zeroizing<String> {
    let capacity = pairs.iter().map(|(k, v)| (k.len() + v.len()) * 3 + 2).sum();
    let mut out = Zeroizing::new(String::with_capacity(capacity));
    for (i, (key, value)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        form_encode(&mut out, key);
        out.push('=');
        form_encode(&mut out, value);
    }
    out
}

fn form_encode(out: &mut String, value: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte))
            }
            _ => {
                out.push('%');
                for nibble in [byte >> 4, byte & 0xf] {
                    out.push(char::from(
                        HEX.get(usize::from(nibble)).copied().unwrap_or(b'0'),
                    ));
                }
            }
        }
    }
}

/// The text of the first `<name>` element, unescaped. STS responses are flat
/// and their values never contain markup, so this reads them without an XML
/// parser.
fn element(xml: &str, name: &str) -> Option<Zeroizing<String>> {
    const ENTITIES: [(&str, char); 5] = [
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&apos;", '\''),
        ("&amp;", '&'),
    ];
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml.get(start..)?.find(&close)?;
    let mut text = xml.get(start..end)?.trim();
    let mut out = Zeroizing::new(String::with_capacity(text.len()));
    while let Some(at) = text.find('&') {
        out.push_str(text.get(..at)?);
        let rest = text.get(at..)?;
        match ENTITIES.iter().find(|(entity, _)| rest.starts_with(entity)) {
            Some((entity, ch)) => {
                out.push(*ch);
                text = rest.get(entity.len()..)?;
            }
            None => {
                out.push('&');
                text = rest.get(1..)?;
            }
        }
    }
    out.push_str(text);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: Option<Zeroizing<String>>) -> Option<String> {
        value.map(|v| v.to_string())
    }

    #[test]
    fn reads_the_credentials_from_an_sts_response() {
        let xml = "<AssumeRoleWithWebIdentityResponse><AssumeRoleWithWebIdentityResult>\
            <Credentials><AccessKeyId>ASIAEXAMPLE</AccessKeyId>\
            <SecretAccessKey>s3cr3t/key+</SecretAccessKey>\
            <SessionToken>tok&amp;en&lt;&bogus;</SessionToken>\
            <Expiration>2026-11-17T12:00:00Z</Expiration></Credentials>\
            </AssumeRoleWithWebIdentityResult></AssumeRoleWithWebIdentityResponse>";
        assert_eq!(
            text(element(xml, "AccessKeyId")).as_deref(),
            Some("ASIAEXAMPLE")
        );
        assert_eq!(
            text(element(xml, "SessionToken")).as_deref(),
            Some("tok&en<&bogus;")
        );
        assert_eq!(text(element(xml, "Missing")), None);
    }

    #[test]
    fn the_form_is_encoded() {
        assert_eq!(
            form(&[("a", "x y"), ("Token", "h.p/s+=")]).as_str(),
            "a=x%20y&Token=h.p%2Fs%2B%3D"
        );
    }

    #[test]
    fn refuses_a_session_name_aws_would_refuse() {
        let tls = crate::host::http_client::default_client_tls_config();
        let arn = "arn:aws:iam::123456789012:role/reports";
        assert!(StsWebIdentity::new(arn, "wl-7f3c", Arc::clone(&tls)).is_ok());
        assert!(StsWebIdentity::new(arn, "has space", Arc::clone(&tls)).is_err());
        assert!(StsWebIdentity::new("not-an-arn", "wl", Arc::clone(&tls)).is_err());
        assert!(
            StsWebIdentity::new(arn, "wl", tls)
                .and_then(|s| s.with_endpoint("http://sts.example"))
                .is_err()
        );
    }

    /// Answer every request with `status` and `xml`, recording each body.
    async fn sts(status: u16, xml: String) -> (StsWebIdentity, Arc<std::sync::Mutex<Vec<String>>>) {
        sts_after(Duration::ZERO, status, xml).await
    }

    /// [`sts`], answering each request `delay` after recording its body.
    async fn sts_after(
        delay: Duration,
        status: u16,
        xml: String,
    ) -> (StsWebIdentity, Arc<std::sync::Mutex<Vec<String>>>) {
        use hyper::service::service_fn;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let server_seen = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let seen = Arc::clone(&server_seen);
                let xml = xml.clone();
                let service = service_fn(move |req: http::Request<hyper::body::Incoming>| {
                    let seen = Arc::clone(&seen);
                    let xml = xml.clone();
                    async move {
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        seen.lock()
                            .unwrap()
                            .push(String::from_utf8_lossy(&body).into_owned());
                        tokio::time::sleep(delay).await;
                        let mut response = http::Response::new(Full::new(bytes::Bytes::from(xml)));
                        *response.status_mut() = http::StatusCode::from_u16(status).unwrap();
                        Ok::<_, std::convert::Infallible>(response)
                    }
                });
                tokio::spawn(
                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service),
                );
            }
        });
        let sts = StsWebIdentity::new(
            "arn:aws:iam::123456789012:role/reports",
            "wl-7f3c",
            crate::host::http_client::default_client_tls_config(),
        )
        .unwrap()
        .with_test_endpoint(format!("http://{addr}/").parse().unwrap());
        (sts, seen)
    }

    fn credentials_xml(expires: chrono::DateTime<chrono::Utc>) -> String {
        format!(
            "<R><Credentials><AccessKeyId>ASIAX</AccessKeyId>\
             <SecretAccessKey>sk</SecretAccessKey><SessionToken>st</SessionToken>\
             <Expiration>{}</Expiration></Credentials></R>",
            expires.to_rfc3339()
        )
    }

    #[tokio::test]
    async fn exchanges_the_token_for_credentials() {
        let (sts, seen) = sts(
            200,
            credentials_xml(chrono::Utc::now() + chrono::Duration::hours(1)),
        )
        .await;
        let credentials = sts.assume(b"header.payload.sig").await.unwrap();
        assert_eq!(credentials.access_key_id, "ASIAX");
        assert_eq!(
            credentials.session_token.as_deref().map(|s| s.as_str()),
            Some("st")
        );
        let sent = seen.lock().unwrap()[0].clone();
        assert!(
            sent.contains("WebIdentityToken=header.payload.sig"),
            "{sent}"
        );
        assert!(sent.contains("RoleSessionName=wl-7f3c"), "{sent}");
    }

    #[tokio::test]
    async fn a_refusal_names_the_sts_error_code() {
        let (sts, _) = sts(
            403,
            "<ErrorResponse><Error><Code>AccessDenied</Code><Message>no</Message></Error></ErrorResponse>"
                .into(),
        )
        .await;
        let err = sts.assume(b"h.p.s").await.unwrap_err().to_string();
        assert!(err.contains("403") && err.contains("AccessDenied"), "{err}");
    }

    #[tokio::test]
    async fn expired_or_incomplete_credentials_are_refused() {
        let (expired, _) = sts(
            200,
            credentials_xml(chrono::Utc::now() - chrono::Duration::minutes(1)),
        )
        .await;
        let err = expired.assume(b"h.p.s").await.unwrap_err().to_string();
        assert!(err.contains("already expired"), "{err}");
        let (partial, _) = sts(
            200,
            "<R><Credentials><AccessKeyId>ASIAX</AccessKeyId></Credentials></R>".into(),
        )
        .await;
        assert!(partial.assume(b"h.p.s").await.is_err());
    }

    #[tokio::test]
    async fn the_refresh_leases_credentials_only_while_they_last() {
        struct Token;
        impl EgressCredentialSource for Token {
            fn current(
                &self,
            ) -> Result<Zeroizing<Vec<u8>>, crate::host::egress_credential::CredentialError>
            {
                Ok(Zeroizing::new(b"h.p.s".to_vec()))
            }
        }
        let (sts, seen) = sts(
            200,
            credentials_xml(chrono::Utc::now() + chrono::Duration::seconds(2)),
        )
        .await;
        let leased = Arc::new(LeasedAwsCredentials::default());
        let task = spawn_web_identity_refresh(sts, Arc::new(Token), Arc::clone(&leased));
        for _ in 0..200 {
            if leased.current().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        use crate::host::egress_sigv4::AwsCredentialSource as _;
        assert_eq!(leased.current().unwrap().access_key_id, "ASIAX");
        task.abort();
        tokio::time::sleep(Duration::from_millis(2100)).await;
        assert!(
            leased.current().is_err(),
            "the lease ends with the credentials"
        );
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[derive(Default)]
    struct RevocableToken(tokio_util::sync::CancellationToken);

    impl EgressCredentialSource for RevocableToken {
        fn current(
            &self,
        ) -> Result<Zeroizing<Vec<u8>>, crate::host::egress_credential::CredentialError> {
            Ok(Zeroizing::new(b"h.p.s".to_vec()))
        }

        fn revocation(&self) -> Option<tokio_util::sync::CancellationToken> {
            Some(self.0.child_token())
        }
    }

    #[tokio::test]
    async fn a_revoked_identity_ends_the_refresh_and_its_credentials() {
        use crate::host::egress_sigv4::AwsCredentialSource as _;
        let (sts, _seen) = sts(
            200,
            credentials_xml(chrono::Utc::now() + chrono::Duration::seconds(600)),
        )
        .await;
        let token = Arc::new(RevocableToken::default());
        let leased = Arc::new(LeasedAwsCredentials::default());
        let task = spawn_web_identity_refresh(sts, token.clone(), Arc::clone(&leased));
        for _ in 0..200 {
            if leased.current().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(leased.current().is_ok());

        token.0.cancel();
        tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .expect("the refresh ends at once")
            .unwrap();
        assert!(leased.current().is_err());
    }

    /// A revoke during an exchange ends it there: what STS answers later is
    /// never served, and the credentials stay closed.
    #[tokio::test]
    async fn a_revoke_mid_exchange_serves_nothing() {
        use crate::host::egress_sigv4::AwsCredentialSource as _;
        let (sts, seen) = sts_after(
            Duration::from_secs(2),
            200,
            credentials_xml(chrono::Utc::now() + chrono::Duration::seconds(600)),
        )
        .await;
        let token = Arc::new(RevocableToken::default());
        let leased = Arc::new(LeasedAwsCredentials::default());
        let task = spawn_web_identity_refresh(sts, token.clone(), Arc::clone(&leased));
        for _ in 0..200 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(seen.lock().unwrap().len(), 1, "the exchange is in flight");

        token.0.cancel();
        tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .expect("the refresh ends without waiting for STS")
            .unwrap();
        assert!(leased.current().is_err());
        leased.renew(
            AwsCredentials {
                access_key_id: "ASIALATE".into(),
                secret_access_key: Zeroizing::new("sk".into()),
                session_token: None,
                expires_at: None,
            },
            Duration::from_secs(60),
        );
        assert!(leased.current().is_err(), "closed for good");
    }
}

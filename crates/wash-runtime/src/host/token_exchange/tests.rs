use super::*;

use std::collections::HashMap;
use std::convert::Infallible;

use hyper::service::service_fn;

struct Seen {
    form: HashMap<String, Vec<String>>,
    dpop: Option<serde_json::Value>,
}

type Reply = (
    http::StatusCode,
    Vec<(&'static str, String)>,
    serde_json::Value,
);

/// A token endpoint answering each request with `reply(n, seen)`.
async fn endpoint<F>(reply: F) -> (http::Uri, Arc<Mutex<Vec<Seen>>>)
where
    F: Fn(usize, &Seen) -> Reply + Send + Sync + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reply = Arc::new(reply);
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let (log, reply) = (Arc::clone(&log), Arc::clone(&reply));
            tokio::spawn(async move {
                let service = service_fn(move |req: http::Request<hyper::body::Incoming>| {
                    let (log, reply) = (Arc::clone(&log), Arc::clone(&reply));
                    async move {
                        let dpop = req
                            .headers()
                            .get("dpop")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|jwt| jwt.split('.').nth(1).map(str::to_string))
                            .and_then(|p| serde_json::from_slice(&B64.decode(p).ok()?).ok());
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        let mut form: HashMap<String, Vec<String>> = HashMap::new();
                        for (k, v) in url::form_urlencoded::parse(&body) {
                            form.entry(k.into_owned()).or_default().push(v.into_owned());
                        }
                        let request = Seen { form, dpop };
                        let mut log = log.lock().unwrap();
                        let (status, headers, json) = reply(log.len(), &request);
                        log.push(request);
                        let mut response = http::Response::new(Full::new(bytes::Bytes::from(
                            serde_json::to_vec(&json).unwrap(),
                        )));
                        *response.status_mut() = status;
                        for (name, value) in headers {
                            response.headers_mut().insert(name, value.parse().unwrap());
                        }
                        Ok::<_, Infallible>(response)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (format!("http://{addr}/token").parse().unwrap(), seen)
}

fn svid() -> Arc<dyn EgressCredentialSource> {
    let lease = LeasedSource::default();
    lease.renew(
        Zeroizing::new(b"svid.jwt.sig".to_vec()),
        Duration::from_secs(60),
    );
    Arc::new(lease)
}

fn exchange(grant: Grant, uri: http::Uri) -> TokenExchange {
    TokenExchange::new(
        "https://idp.example/token",
        grant,
        svid(),
        crate::host::http_client::default_client_tls_config(),
    )
    .unwrap()
    .with_test_endpoint(uri)
}

fn ok(token_type: &str) -> Reply {
    (
        http::StatusCode::OK,
        Vec::new(),
        serde_json::json!({
            "access_token": "at-1",
            "token_type": token_type,
            "expires_in": 600,
            "issued_token_type": TOKEN_TYPE_ACCESS,
        }),
    )
}

fn jwt(claims: serde_json::Value) -> String {
    format!(
        "{}.{}.sig",
        B64.encode(r#"{"alg":"none"}"#),
        B64.encode(serde_json::to_vec(&claims).unwrap())
    )
}

#[tokio::test]
async fn token_exchange_presents_the_svid_and_every_requested_parameter() {
    let (uri, seen) = endpoint(|_, _| ok("Bearer")).await;
    let issued = exchange(Grant::TokenExchange, uri)
        .with_audience("https://api.example")
        .with_resource("https://api.example/v1")
        .unwrap()
        .with_scope("read write")
        .with_authorization_details(serde_json::json!([{"type": "payment", "amount": 5}]))
        .unwrap()
        .exchange()
        .await
        .unwrap();
    assert_eq!(issued.access_token.as_str(), "at-1");
    assert_eq!(issued.token_type, TokenType::Bearer);
    assert!(
        issued.lifetime > Duration::from_secs(590) && issued.lifetime <= Duration::from_secs(600)
    );

    let seen = seen.lock().unwrap();
    let form = &seen[0].form;
    let one = |k: &str| form[k][0].as_str();
    assert_eq!(one("grant_type"), TOKEN_EXCHANGE);
    assert_eq!(one("subject_token"), "svid.jwt.sig");
    assert_eq!(one("subject_token_type"), TOKEN_TYPE_JWT);
    assert_eq!(one("requested_token_type"), TOKEN_TYPE_ACCESS);
    assert_eq!(one("audience"), "https://api.example");
    assert_eq!(one("resource"), "https://api.example/v1");
    assert_eq!(one("scope"), "read write");
    let details: serde_json::Value = serde_json::from_str(one("authorization_details")).unwrap();
    assert_eq!(details[0]["type"], "payment");
}

#[tokio::test]
async fn the_jwt_grants_present_the_svid_as_grant_or_client_assertion() {
    let (uri, seen) = endpoint(|_, _| ok("bearer")).await;
    exchange(Grant::JwtBearer, uri.clone())
        .with_audience("ignored")
        .exchange()
        .await
        .unwrap();
    exchange(Grant::ClientAssertion, uri)
        .with_client_id("svc")
        .exchange()
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].form["grant_type"], [JWT_BEARER]);
    assert_eq!(seen[0].form["assertion"], ["svid.jwt.sig"]);
    assert!(!seen[0].form.contains_key("audience"));
    assert_eq!(seen[1].form["grant_type"], ["client_credentials"]);
    assert_eq!(
        seen[1].form["client_assertion_type"],
        [CLIENT_ASSERTION_JWT]
    );
    assert_eq!(seen[1].form["client_assertion"], ["svid.jwt.sig"]);
    assert_eq!(seen[1].form["client_id"], ["svc"]);
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Serves a new SVID on every read, as a source minting fresh assertions does.
#[derive(Default)]
struct Minting(std::sync::atomic::AtomicUsize);

impl EgressCredentialSource for Minting {
    fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        let n = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Zeroizing::new(format!("svid-{n}").into_bytes()))
    }
}

#[tokio::test]
async fn a_token_that_is_not_what_was_asked_for_is_refused() {
    let good = jwt(serde_json::json!({"aud": "api"}));
    let later = now() + 3600;
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "wrong type",
            serde_json::json!({"access_token": good, "token_type": "mac", "issued_token_type": TOKEN_TYPE_ACCESS}),
        ),
        (
            "no issued type",
            serde_json::json!({"access_token": good, "token_type": "Bearer"}),
        ),
        (
            "refresh token",
            serde_json::json!({"access_token": good, "token_type": "Bearer", "issued_token_type": "urn:ietf:params:oauth:token-type:refresh_token"}),
        ),
        (
            "zero lifetime",
            serde_json::json!({"access_token": good, "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS, "expires_in": 0}),
        ),
        (
            "fractional lifetime",
            serde_json::json!({"access_token": good, "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS, "expires_in": 1.5}),
        ),
        (
            "unparseable lifetime",
            serde_json::json!({"access_token": good, "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS, "expires_in": "soon"}),
        ),
        (
            "too long",
            serde_json::json!({"access_token": good, "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS, "expires_in": 7200}),
        ),
        (
            "extra scope",
            serde_json::json!({"access_token": good, "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS, "scope": "read admin"}),
        ),
        (
            "no token",
            serde_json::json!({"token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS}),
        ),
        (
            "wrong audience",
            serde_json::json!({"access_token": jwt(serde_json::json!({"aud": "other"})), "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS}),
        ),
        (
            "expired",
            serde_json::json!({"access_token": jwt(serde_json::json!({"aud": "api", "exp": 1})), "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS}),
        ),
        (
            "unparseable exp",
            serde_json::json!({"access_token": jwt(serde_json::json!({"aud": "api", "exp": "tomorrow"})), "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS}),
        ),
        (
            "not yet valid",
            serde_json::json!({"access_token": jwt(serde_json::json!({"aud": "api", "nbf": later})), "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS}),
        ),
        (
            "unparseable nbf",
            serde_json::json!({"access_token": jwt(serde_json::json!({"aud": "api", "nbf": null})), "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS}),
        ),
    ];
    let accepted = serde_json::json!({"access_token": good, "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS, "scope": "read"});
    for (name, body) in [("control", accepted)].into_iter().chain(cases) {
        let (uri, _) = endpoint(move |_, _| (http::StatusCode::OK, Vec::new(), body.clone())).await;
        let result = exchange(Grant::TokenExchange, uri)
            .with_scope("read")
            .expecting_audience("api")
            .with_lifetimes(Duration::from_secs(3600), Duration::from_secs(60))
            .exchange()
            .await;
        assert_eq!(result.is_ok(), name == "control", "{name}");
    }
}

#[tokio::test]
async fn a_jwt_lease_ends_at_its_exp_and_matches_a_listed_audience() {
    let exp = now() as f64 + 120.5;
    let token = jwt(serde_json::json!({"aud": ["x", "api"], "exp": exp, "nbf": now() + 10}));
    let (uri, _) = endpoint(move |_, _| {
        (
            http::StatusCode::OK,
            Vec::new(),
            serde_json::json!({"access_token": token, "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS, "expires_in": 3600}),
        )
    })
    .await;
    let issued = exchange(Grant::TokenExchange, uri)
        .expecting_audience("api")
        .exchange()
        .await
        .unwrap();
    assert!(
        issued.lifetime > Duration::from_secs(100)
            && issued.lifetime <= Duration::from_millis(120_500)
    );
}

#[tokio::test]
async fn dpop_retries_once_with_the_servers_nonce_and_refuses_a_bearer_downgrade() {
    let (uri, seen) = endpoint(|n, seen| {
        if n == 0 {
            (
                http::StatusCode::BAD_REQUEST,
                vec![("dpop-nonce", "n-42".to_string())],
                serde_json::json!({"error": "use_dpop_nonce"}),
            )
        } else {
            assert_eq!(seen.dpop.as_ref().unwrap()["nonce"], "n-42");
            ok("DPoP")
        }
    })
    .await;
    let key = Arc::new(DpopKey::generate().unwrap());
    let issued = TokenExchange::new(
        "https://idp.example/token",
        Grant::JwtBearer,
        Arc::new(Minting::default()),
        crate::host::http_client::default_client_tls_config(),
    )
    .unwrap()
    .with_test_endpoint(uri)
    .with_dpop(Arc::clone(&key))
    .exchange()
    .await
    .unwrap();
    assert_eq!(issued.token_type, TokenType::Dpop);
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].form["assertion"], ["svid-0"]);
        assert_eq!(seen[1].form["assertion"], ["svid-1"]);
        let first = seen[0].dpop.as_ref().unwrap();
        assert_eq!(first["htm"], "POST");
        assert!(first.get("ath").is_none());
    }

    let (uri, _) = endpoint(|_, _| ok("Bearer")).await;
    let downgraded = exchange(Grant::JwtBearer, uri)
        .with_dpop(key)
        .exchange()
        .await;
    assert!(downgraded.is_err());
}

#[tokio::test]
async fn a_dpop_jwt_must_be_bound_to_this_key() {
    let key = Arc::new(DpopKey::generate().unwrap());
    let cases = [
        (
            "control",
            serde_json::json!({"cnf": {"jkt": key.thumbprint()}}),
        ),
        (
            "another key",
            serde_json::json!({"cnf": {"jkt": "someone-else"}}),
        ),
        ("unbound", serde_json::json!({"sub": "svc"})),
    ];
    for (name, claims) in cases {
        let body = serde_json::json!({"access_token": jwt(claims), "token_type": "DPoP"});
        let (uri, _) = endpoint(move |_, _| (http::StatusCode::OK, Vec::new(), body.clone())).await;
        let result = exchange(Grant::JwtBearer, uri)
            .with_dpop(Arc::clone(&key))
            .exchange()
            .await;
        assert_eq!(result.is_ok(), name == "control", "{name}");
    }
}

#[test]
fn refresh_lands_before_expiry_and_failures_back_off_with_jitter() {
    assert_eq!(
        refresh_after(Duration::from_secs(600)),
        Duration::from_secs(480)
    );
    assert_eq!(
        refresh_after(Duration::from_secs(60)),
        Duration::from_secs(30)
    );
    assert_eq!(
        refresh_after(Duration::from_secs(10)),
        Duration::from_secs(5)
    );
    assert_eq!(
        refresh_after(Duration::from_millis(500)),
        Duration::from_secs(1)
    );
    for failures in 1..40 {
        let ceiling = (RETRY * 2u32.pow((failures - 1).min(16))).min(MAX_BACKOFF);
        let wait = backoff(failures);
        assert!(
            wait >= ceiling / 2 && wait <= ceiling,
            "{failures}: {wait:?}"
        );
    }
    assert!(backoff(30) >= MAX_BACKOFF / 2);
}

#[tokio::test]
async fn the_refresh_task_leases_the_token_it_was_issued() {
    let (uri, _) = endpoint(|_, _| ok("Bearer")).await;
    let exchange = Arc::new(exchange(Grant::TokenExchange, uri));
    assert!(exchange.current().is_err());
    let task = spawn_token_refresh(Arc::clone(&exchange)).unwrap();
    assert!(
        spawn_token_refresh(Arc::clone(&exchange)).is_err(),
        "an exchange is refreshed once"
    );
    for _ in 0..100 {
        if exchange.current().is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(exchange.current().unwrap().as_slice(), b"at-1");
    assert_eq!(exchange.token_type(), Some(TokenType::Bearer));
    drop(task);
    assert!(
        exchange.current().is_err(),
        "dropping the refresh forgets the token"
    );
    exchange
        .held
        .renew(Zeroizing::new(b"late".to_vec()), Duration::from_secs(60));
    assert!(
        exchange.current().is_err(),
        "a renewal racing the drop stores nothing"
    );
    assert!(spawn_token_refresh(Arc::clone(&exchange)).is_err());
}

#[tokio::test]
async fn expires_in_may_be_a_string_and_counts_from_before_the_request() {
    let (uri, _) = endpoint(|_, _| {
        std::thread::sleep(Duration::from_millis(1100));
        (
            http::StatusCode::OK,
            Vec::new(),
            serde_json::json!({"access_token": "at-1", "token_type": "Bearer", "issued_token_type": TOKEN_TYPE_ACCESS, "expires_in": "60"}),
        )
    })
    .await;
    let issued = exchange(Grant::TokenExchange, uri)
        .exchange()
        .await
        .unwrap();
    assert!(
        issued.lifetime < Duration::from_secs(59),
        "{:?}",
        issued.lifetime
    );
    assert!(issued.lifetime > Duration::from_secs(50));
}

#[test]
fn configuration_refuses_what_the_rfcs_forbid() {
    let tls = crate::host::http_client::default_client_tls_config();
    assert!(
        TokenExchange::new(
            "http://idp.example/token",
            Grant::JwtBearer,
            svid(),
            Arc::clone(&tls)
        )
        .is_err()
    );
    let ok = || {
        TokenExchange::new(
            "https://idp.example/token",
            Grant::JwtBearer,
            svid(),
            Arc::clone(&tls),
        )
        .unwrap()
    };
    assert!(ok().with_resource("https://api.example/#frag").is_err());
    assert!(ok().with_resource("relative/path").is_err());
    assert!(
        ok().with_authorization_details(serde_json::json!({"type": "x"}))
            .is_err()
    );
    assert!(
        ok().with_authorization_details(serde_json::json!([{"no_type": 1}]))
            .is_err()
    );
}

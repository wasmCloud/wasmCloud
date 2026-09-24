//! Integration test: a host component plugin runs client TLS over its own
//! `wasi:sockets` connection, with trust from its `allowedHosts` grant.
//!
//! `tls-plugin-p3` exports `acme:tlsprobe/probe`, which dials a local rustls
//! echo server whose certificate a private CA issued, and handshakes through
//! `wasmcloud:tls` or `wasi:tls` — the host serves both from one
//! implementation. The `tls-plugin-caller-p3` workload drives it over HTTP.

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::time::timeout;

use wash_runtime::engine::Engine;
use wash_runtime::host::allowed_loopback::AllowedLoopbackPort;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::component_host::ComponentHostPlugin;
use wash_runtime::plugin::{PluginAllowedHost, PluginTlsPolicy, TlsGrant, TlsRoots};
use wash_runtime::sockets::policy::SocketPolicy;
use wash_runtime::types::LocalResources;
use wash_runtime::wit::WitInterface;

mod common;
use common::{component_workload_request, http_incoming_handler_interface};

const TLS_PLUGIN_WASM: &[u8] = include_bytes!("wasm/tls_plugin_p3.wasm");
const CALLER_WASM: &[u8] = include_bytes!("wasm/tls_plugin_caller_p3.wasm");
const NO_TLS_PLUGIN_WASM: &[u8] = include_bytes!("wasm/kv_plugin.wasm");
const HTTP_PLUGIN_WASM: &[u8] = include_bytes!("wasm/http_egress_plugin.wasm");
const HTTP_CALLER_WASM: &[u8] = include_bytes!("wasm/http_egress_plugin_caller.wasm");
const PLUGIN_ID: &str = "tls-plugin";
const SERVER_NAME: &str = "cluster.internal";

/// A private CA, and PEM files for it and for a client certificate it issued.
struct Pki {
    dir: tempfile::TempDir,
    ca_der: CertificateDer<'static>,
    server_chain: Vec<CertificateDer<'static>>,
    server_key: PrivateKeyDer<'static>,
}

impl Pki {
    fn new() -> Self {
        wash_runtime::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().unwrap();
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::from_params(&ca_params, &ca_key);

        let server_key = KeyPair::generate().unwrap();
        let server_cert =
            CertificateParams::new(vec![SERVER_NAME.to_string(), "127.0.0.1".to_string()])
                .unwrap()
                .signed_by(&server_key, &issuer)
                .unwrap();
        let client_key = KeyPair::generate().unwrap();
        let client_cert = CertificateParams::new(vec!["tls-plugin".to_string()])
            .unwrap()
            .signed_by(&client_key, &issuer)
            .unwrap();
        std::fs::write(dir.path().join("ca.crt"), ca_cert.pem()).unwrap();
        std::fs::write(dir.path().join("client.crt"), client_cert.pem()).unwrap();
        std::fs::write(dir.path().join("client.key"), client_key.serialize_pem()).unwrap();

        Self {
            ca_der: ca_cert.der().clone(),
            server_chain: vec![server_cert.der().clone()],
            server_key: PrivateKeyDer::from_pem_slice(server_key.serialize_pem().as_bytes())
                .unwrap(),
            dir,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn ca_grant(&self) -> TlsGrant {
        TlsGrant {
            ca: Some(self.path("ca.crt")),
            roots: Some(TlsRoots::Replace),
            ..Default::default()
        }
    }

    fn mtls_grant(&self) -> TlsGrant {
        TlsGrant {
            client_cert: Some(self.path("client.crt")),
            client_key: Some(self.path("client.key")),
            ..self.ca_grant()
        }
    }

    fn server_config(&self, mtls: bool) -> Arc<rustls::ServerConfig> {
        let builder = rustls::ServerConfig::builder();
        let builder = if mtls {
            let mut roots = rustls::RootCertStore::empty();
            roots.add(self.ca_der.clone()).unwrap();
            builder.with_client_cert_verifier(
                rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
                    .build()
                    .unwrap(),
            )
        } else {
            builder.with_no_client_auth()
        };
        Arc::new(
            builder
                .with_single_cert(self.server_chain.clone(), self.server_key.clone_key())
                .unwrap(),
        )
    }
}

/// How a test server answers what it reads.
#[derive(Clone, Copy)]
enum Reply {
    /// `PONG\r\n` once a line arrives.
    Pong,
    /// Every byte back as it arrives, so the client's writes meet
    /// backpressure while it reads.
    Echo,
}

/// A TLS server on host loopback that answers per `reply`, then waits for the
/// client's `close_notify` before sending its own, so a client can check that
/// both directions closed cleanly. Returns the address a guest dials, through
/// the host sentinel.
async fn start_server(config: Arc<rustls::ServerConfig>, reply: Reply) -> Result<SocketAddr> {
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await?;
    let port = listener.local_addr()?.port();
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut tail = Vec::new();
                let mut ponged = false;
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    let n = match tls.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    let chunk = buf.get(..n).unwrap_or_default();
                    match reply {
                        Reply::Echo => {
                            if tls.write_all(chunk).await.is_err() {
                                return;
                            }
                        }
                        Reply::Pong if !ponged => {
                            // Only the last two bytes can complete a CRLF split
                            // across reads, so there is no need to keep more.
                            tail.extend_from_slice(chunk);
                            if tail.windows(2).any(|w| w == b"\r\n") {
                                ponged = true;
                                if tls.write_all(b"PONG\r\n").await.is_err() {
                                    return;
                                }
                            }
                            let keep = tail.len().saturating_sub(1);
                            tail.drain(..keep);
                        }
                        Reply::Pong => {}
                    }
                }
                let _ = tls.shutdown().await;
            });
        }
    });
    Ok(SocketAddr::new(
        IpAddr::V4(wash_runtime::sockets::internal_names::HOST_SENTINEL),
        port,
    ))
}

async fn start_echo(config: Arc<rustls::ServerConfig>) -> Result<SocketAddr> {
    start_server(config, Reply::Pong).await
}

fn probe_interface() -> WitInterface {
    WitInterface {
        namespace: "acme".to_string(),
        package: "tlsprobe".to_string(),
        interfaces: ["probe".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

fn loopback_policy() -> Arc<SocketPolicy> {
    Arc::new(SocketPolicy {
        host_loopback_enabled: true,
        ..Default::default()
    })
}

fn tls_policy(grant: TlsGrant) -> Arc<PluginTlsPolicy> {
    Arc::new(
        PluginTlsPolicy::from_grants(&[SERVER_NAME, "127.0.0.1"].map(|host| PluginAllowedHost {
            host: host.parse().unwrap(),
            tls: Some(grant.clone()),
        }))
        .unwrap()
        .unwrap(),
    )
}

/// A host with `tls-plugin-p3` granted `echo_port` on host loopback and
/// `grant` for [`SERVER_NAME`], plus the caller workload.
async fn start_host(echo_port: u16, grant: TlsGrant) -> Result<(SocketAddr, impl HostApi)> {
    start_host_with_plugin(
        echo_port,
        grant,
        TLS_PLUGIN_WASM,
        CALLER_WASM,
        probe_interface(),
    )
    .await
}

async fn start_host_with_plugin(
    echo_port: u16,
    grant: TlsGrant,
    plugin_wasm: &[u8],
    caller_wasm: &'static [u8],
    interface: WitInterface,
) -> Result<(SocketAddr, impl HostApi)> {
    let engine = Engine::builder()
        .with_socket_policy(loopback_policy())
        .build()?;
    let ingress = Ingress::builder(DevRouter::default(), "127.0.0.1:0".parse()?)
        .build()
        .await?;
    let addr = ingress.addr();
    let builder = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(ingress));
    let plugin = ComponentHostPlugin::builder()
        .id(PLUGIN_ID)
        .wasm(plugin_wasm)
        .engine(engine)
        .native_plugins(builder.native_plugins())
        .allowed_hosts(Arc::from(["*".parse()?]))
        .allowed_host_loopback_ports(Arc::from([AllowedLoopbackPort::tcp(echo_port)]))
        .socket_policy(loopback_policy())
        .tls_policy(tls_policy(grant))
        .maybe_host_ref(Some(builder.host_ref()))
        .build()
        .await
        .context("tls-plugin should link cleanly")?;
    let host = builder
        .with_plugin(Arc::new(plugin))?
        .build()?
        .start()
        .await?;
    host.workload_start(component_workload_request(
        "tls-plugin-caller",
        "caller",
        caller_wasm,
        LocalResources::default(),
        vec![http_incoming_handler_interface("caller", None), interface],
    ))
    .await?;
    Ok((addr, host))
}

async fn ping(ingress: SocketAddr, echo: SocketAddr, name: &str, via: &str) -> Result<String> {
    let resp = timeout(
        Duration::from_secs(20),
        reqwest::Client::new()
            .get(format!(
                "http://{ingress}/ping?addr={echo}&name={name}&via={via}"
            ))
            .header("HOST", "caller")
            .send(),
    )
    .await
    .context("request timed out")??;
    Ok(resp.text().await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_interfaces_handshake_with_the_granted_private_ca() -> Result<()> {
    let pki = Pki::new();
    let echo = start_echo(pki.server_config(false)).await?;
    let (ingress, _host) = start_host(echo.port(), pki.ca_grant()).await?;
    for via in ["wasmcloud", "wasi"] {
        assert_eq!(
            ping(ingress, echo, SERVER_NAME, via).await?,
            "PONG\r\n",
            "via {via}"
        );
    }
    Ok(())
}

/// A megabyte each way at once, through a server that echoes as it reads: the
/// send and receive halves of one session both sit under backpressure, and
/// both of their result futures must report a clean close.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_interfaces_carry_full_duplex_traffic_under_backpressure() -> Result<()> {
    let pki = Pki::new();
    let echo = start_server(pki.server_config(false), Reply::Echo).await?;
    let (ingress, _host) = start_host(echo.port(), pki.ca_grant()).await?;
    let mut expected = "x".repeat(1024 * 1024);
    expected.push_str("\r\n");
    for via in ["wasmcloud", "wasi"] {
        let reply = ping(ingress, echo, SERVER_NAME, via).await?;
        assert!(
            reply == expected,
            "via {via}: {} bytes back, starting {:?}",
            reply.len(),
            reply.get(..reply.len().min(120)).unwrap_or_default()
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_granted_client_certificate_satisfies_mtls() -> Result<()> {
    let pki = Pki::new();
    let echo = start_echo(pki.server_config(true)).await?;

    let (ingress, _host) = start_host(echo.port(), pki.mtls_grant()).await?;
    assert_eq!(
        ping(ingress, echo, SERVER_NAME, "wasmcloud").await?,
        "PONG\r\n"
    );

    let (ingress, _host) = start_host(echo.port(), pki.ca_grant()).await?;
    let reply = ping(ingress, echo, SERVER_NAME, "wasmcloud").await?;
    assert_ne!(
        reply, "PONG\r\n",
        "a server requiring a client certificate must refuse none"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_name_without_a_tls_grant_is_refused_before_the_handshake() -> Result<()> {
    let pki = Pki::new();
    let echo = start_echo(pki.server_config(false)).await?;
    let (ingress, _host) = start_host(echo.port(), pki.ca_grant()).await?;
    for via in ["wasmcloud", "wasi"] {
        let reply = ping(ingress, echo, "other.internal", via).await?;
        assert!(
            reply.starts_with("error: connect:") && reply.contains("declares `tls`"),
            "via {via}: {reply}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_the_granted_roots_do_not_cover_fails_the_handshake() -> Result<()> {
    let pki = Pki::new();
    let echo = start_echo(pki.server_config(false)).await?;
    // Public roots only: the private CA that issued the server's certificate
    // is not among them.
    let (ingress, _host) = start_host(echo.port(), TlsGrant::default()).await?;
    let reply = ping(ingress, echo, SERVER_NAME, "wasi").await?;
    assert!(
        reply.contains("handshake") && reply.contains("UnknownIssuer"),
        "got: {reply}"
    );
    Ok(())
}

#[tokio::test]
async fn a_plugin_that_cannot_apply_declared_tls_fails_to_load() -> Result<()> {
    let pki = Pki::new();
    let engine = Engine::builder().build()?;
    let Err(err) = ComponentHostPlugin::builder()
        .id("no-tls-plugin")
        .wasm(NO_TLS_PLUGIN_WASM)
        .engine(engine)
        .tls_policy(tls_policy(pki.ca_grant()))
        .build()
        .await
    else {
        panic!("a plugin without HTTP or TLS imports must refuse a `tls` declaration");
    };
    let err = format!("{err:#}");
    assert!(
        err.contains("no supported HTTP or TLS client"),
        "got: {err}"
    );
    Ok(())
}

async fn request(ingress: SocketAddr, path: &str) -> Result<String> {
    timeout(Duration::from_secs(20), async {
        Ok(reqwest::Client::new()
            .get(format!("http://{ingress}{path}"))
            .header("HOST", "caller")
            .send()
            .await?
            .text()
            .await?)
    })
    .await
    .context("request timed out")?
}

async fn start_https(mut config: Arc<rustls::ServerConfig>) -> Result<SocketAddr> {
    Arc::make_mut(&mut config).alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(socket).await else {
                    return;
                };
                let service =
                    hyper::service::service_fn(|_: hyper::Request<hyper::body::Incoming>| async {
                        Ok::<_, std::convert::Infallible>(hyper::Response::new(
                            http_body_util::Full::new(bytes::Bytes::from_static(b"ok")),
                        ))
                    });
                let _ = hyper_util::server::conn::auto::Builder::new(
                    hyper_util::rt::TokioExecutor::new(),
                )
                .serve_connection(hyper_util::rt::TokioIo::new(tls), service)
                .await;
            });
        }
    });
    Ok(addr)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn required_tls_dialer_connects_but_raw_sockets_are_denied() -> Result<()> {
    let pki = Pki::new();
    let echo = start_echo(pki.server_config(true)).await?;
    let (ingress, _host) = start_host(
        echo.port(),
        TlsGrant {
            required: true,
            ..pki.mtls_grant()
        },
    )
    .await?;
    assert_eq!(request(ingress, "/raw").await?, "true");
    assert_eq!(
        request(
            ingress,
            &format!("/dial?endpoint=tls://127.0.0.1:{}", echo.port())
        )
        .await?,
        "PONG\r\n"
    );
    let denied = request(
        ingress,
        &format!("/http?endpoint=http://127.0.0.1:{}", echo.port()),
    )
    .await?;
    assert!(denied.contains("HttpRequestDenied"), "{denied}");
    let denied = request(ingress, "/dial?endpoint=tls://127.0.0.1:1").await?;
    assert!(denied.contains("allowedHostLoopbackPorts"), "{denied}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p3_https_and_grpc_use_the_grants_identity_and_roots() -> Result<()> {
    let pki = Pki::new();
    let server = start_https(pki.server_config(true)).await?;
    let (ingress, _host) = start_host(
        server.port(),
        TlsGrant {
            required: true,
            ..pki.mtls_grant()
        },
    )
    .await?;
    for grpc in [false, true] {
        assert_eq!(
            request(
                ingress,
                &format!("/http?endpoint=https://{server}&grpc={grpc}")
            )
            .await?,
            "200"
        );
        let denied = request(
            ingress,
            &format!("/http?endpoint=http://{server}&grpc={grpc}"),
        )
        .await?;
        assert!(denied.contains("HttpRequestDenied"), "{denied}");
    }
    let (ingress, _host) = start_host(server.port(), pki.ca_grant()).await?;
    assert_ne!(
        request(ingress, &format!("/http?endpoint=https://{server}")).await?,
        "200"
    );
    let (ingress, _host) = start_host(server.port(), TlsGrant::default()).await?;
    assert_ne!(
        request(ingress, &format!("/http?endpoint=https://{server}")).await?,
        "200"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_only_p2_plugin_uses_the_grant_without_a_tls_import() -> Result<()> {
    let pki = Pki::new();
    let server = start_https(pki.server_config(true)).await?;
    let (ingress, _host) = start_host_with_plugin(
        server.port(),
        TlsGrant {
            required: true,
            ..pki.mtls_grant()
        },
        HTTP_PLUGIN_WASM,
        HTTP_CALLER_WASM,
        WitInterface::from("acme:httpegress/fetch@0.1.0"),
    )
    .await?;
    assert_eq!(
        request(ingress, &format!("/fetch?host=https://{server}")).await?,
        "200"
    );
    assert_eq!(
        request(ingress, &format!("/fetch?host={server}")).await?,
        "403"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_plugin_tls_cannot_reuse_an_authenticated_http_connection() -> Result<()> {
    use http_body_util::BodyExt as _;
    use wash_runtime::host::http::HostHandler as _;
    let pki = Pki::new();
    let server = start_https(pki.server_config(true)).await?;
    let ingress = Ingress::builder(DevRouter::default(), "127.0.0.1:0".parse()?)
        .build()
        .await?;
    let authenticated = tls_policy(pki.mtls_grant());
    for policy in [authenticated.clone(), authenticated] {
        let request = hyper::Request::builder()
            .uri(format!("https://{server}/"))
            .body(wasmtime_wasi_http::WasiBody::default())?;
        let (response, _) = Box::into_pin(ingress.outgoing_plugin_request(
            "plugin",
            request,
            None,
            Box::new(async { Ok(()) }),
            &["*".parse()?],
            policy,
        ))
        .await?;
        response.into_body().collect().await?;
    }
    let request = hyper::Request::builder()
        .uri(format!("https://{server}/"))
        .body(wasmtime_wasi_http::WasiBody::default())?;
    let result = Box::into_pin(ingress.outgoing_plugin_request(
        "plugin",
        request,
        None,
        Box::new(async { Ok(()) }),
        &["*".parse()?],
        tls_policy(pki.ca_grant()),
    ))
    .await;
    assert!(
        result.is_err(),
        "a new policy must not inherit the old client's authenticated connection"
    );
    Ok(())
}

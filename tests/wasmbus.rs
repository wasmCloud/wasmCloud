use std::collections::HashMap;
use std::env;
use std::env::consts::{ARCH, FAMILY, OS};
use std::net::Ipv6Addr;
use std::pin::pin;
use std::process::ExitStatus;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context};
use nkeys::KeyPair;
use redis::ConnectionLike;
use serde::Deserialize;
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tokio::{fs, select, spawn, try_join};
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt;
use tracing::warn;
use tracing_subscriber::prelude::*;
use url::Url;
use uuid::Uuid;
use wascap::jwt;
use wascap::wasm::extract_claims;
use wasmcloud_control_interface::{
    ActorAuctionAck, ActorDescription, ActorInstance, ClientBuilder, CtlOperationAck,
    Host as HostInfo, HostInventory, ProviderAuctionAck,
};
use wasmcloud_host::wasmbus::{Host, HostConfig};

async fn free_port() -> anyhow::Result<u16> {
    let lis = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
        .await
        .context("failed to start TCP listener")?
        .local_addr()
        .context("failed to query listener local address")?;
    Ok(lis.port())
}

async fn assert_start_provider(
    client: &wasmcloud_control_interface::Client,
    nats_client: &async_nats::Client, // TODO: This should be exposed by `wasmcloud_control_interface::Client`
    lattice_prefix: &str,
    host_key: &KeyPair,
    provider_key: &KeyPair,
    url: impl AsRef<str>,
    configuration: Option<String>,
) -> anyhow::Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct HealthCheckResponse {
        #[serde(default)]
        healthy: bool,
        #[serde(default)]
        message: Option<String>,
    }

    let CtlOperationAck { accepted, error } = client
        .start_provider(
            &host_key.public_key(),
            url.as_ref(),
            None,
            None,
            configuration,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to start provider"))?;
    ensure!(error == "");
    ensure!(accepted);

    let res = pin!(IntervalStream::new(interval(Duration::from_secs(1)))
        .take(30)
        .then(|_| nats_client.request(
            format!(
                "wasmbus.rpc.{}.{}.default.health",
                lattice_prefix,
                provider_key.public_key()
            ),
            "".into(),
        ))
        .filter_map(|res| {
            match res {
                Err(error) => {
                    warn!(?error, "failed to connect to provider");
                    None
                }
                Ok(res) => Some(res),
            }
        }))
    .next()
    .await
    .context("failed to perform health check request")?;

    let HealthCheckResponse { healthy, message } =
        rmp_serde::from_slice(&res.payload).context("failed to decode health check response")?;
    ensure!(message == None);
    ensure!(healthy);
    Ok(())
}

async fn assert_advertise_link(
    client: &wasmcloud_control_interface::Client,
    actor_claims: &jwt::Claims<jwt::Actor>,
    provider_key: &KeyPair,
    contract_id: impl AsRef<str>,
    values: HashMap<String, String>,
) -> anyhow::Result<()> {
    let CtlOperationAck { accepted, error } = client
        .advertise_link(
            &actor_claims.subject,
            &provider_key.public_key(),
            contract_id.as_ref(),
            "default",
            values,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to advertise link"))?;
    ensure!(error == "");
    ensure!(accepted);
    Ok(())
}

async fn assert_remove_link(
    client: &wasmcloud_control_interface::Client,
    actor_claims: &jwt::Claims<jwt::Actor>,
    contract_id: impl AsRef<str>,
) -> anyhow::Result<()> {
    let CtlOperationAck { accepted, error } = client
        .remove_link(&actor_claims.subject, contract_id.as_ref(), "default")
        .await
        .map_err(|e| anyhow!(e).context("failed to remove link"))?;
    ensure!(error == "");
    ensure!(accepted);
    Ok(())
}

async fn spawn_server(
    cmd: &mut Command,
) -> anyhow::Result<(JoinHandle<anyhow::Result<ExitStatus>>, oneshot::Sender<()>)> {
    let mut child = cmd
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn child")?;
    let (stop_tx, stop_rx) = oneshot::channel();
    let child = spawn(async move {
        select!(
            res = stop_rx => {
                res.context("failed to wait for shutdown")?;
                child.kill().await.context("failed to kill child")?;
                child.wait().await
            }
            status = child.wait() => {
                status
            }
        )
        .context("failed to wait for child")
    });
    Ok((child, stop_tx))
}

#[tokio::test(flavor = "multi_thread")]
async fn wasmbus() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let nats_port = free_port().await?;
    let ctl_nats_url = Url::parse(&format!("nats://localhost:{nats_port}"))
        .context("failed to parse control NATS URL")?;
    let jetstream_dir = tempdir().context("failed to create temporary directory")?;
    let (nats_server, stop_nats_tx) = spawn_server(
        Command::new(
            env::var("WASMCLOUD_NATS")
                .as_ref()
                .map(String::as_str)
                .unwrap_or("nats-server"),
        )
        .args(["-js", "-V", "-T=false", "-p", &nats_port.to_string(), "-sd"])
        .arg(jetstream_dir.path()),
    )
    .await
    .context("failed to start NATS")?;
    let ctl_nats_client = async_nats::connect_with_options(
        ctl_nats_url.as_str(),
        async_nats::ConnectOptions::new().retry_on_initial_connect(),
    )
    .await
    .context("failed to connect to NATS control server")?;

    let nats_client = ctl_nats_client; // FIXME: we should be using separate NATS clients for CTL, RPC, and PROV_RPC

    let redis_port = free_port().await?;
    let redis_url = Url::parse(&format!("redis://localhost:{redis_port}"))
        .context("failed to parse Redis URL")?;
    let (redis_server, stop_redis_tx) = spawn_server(
        Command::new(
            env::var("WASMCLOUD_REDIS")
                .as_ref()
                .map(String::as_str)
                .unwrap_or("redis-server"),
        )
        .args(["--port", &redis_port.to_string()]),
    )
    .await
    .context("failed to start Redis")?;
    let mut redis_client =
        redis::Client::open(redis_url.as_str()).context("failed to connect to Redis")?;

    const TEST_PREFIX: &str = "test-prefix";
    let ctl_client = ClientBuilder::new(nats_client.clone())
        .lattice_prefix(TEST_PREFIX.to_string())
        .build()
        .await
        .map_err(|e| anyhow!(e).context("failed to build control interface client"))?;

    let cluster_key = KeyPair::new_cluster();
    let host_key = KeyPair::new_server();

    env::set_var("HOST_PATH", "test-path");
    let expected_labels = HashMap::from([
        ("hostcore.arch".into(), ARCH.into()),
        ("hostcore.os".into(), OS.into()),
        ("hostcore.osfamily".into(), FAMILY.into()),
        ("path".into(), "test-path".into()),
    ]);

    let (host, shutdown) = Host::new(HostConfig {
        ctl_nats_url: ctl_nats_url.clone(),
        lattice_prefix: TEST_PREFIX.to_string(),
        cluster_seed: Some(cluster_key.seed().unwrap()),
        host_seed: Some(host_key.seed().unwrap()),
    })
    .await
    .context("failed to initialize host")?;

    let mut hosts = ctl_client
        .get_hosts()
        .await
        .map_err(|e| anyhow!(e).context("failed to get hosts"))?;
    match (hosts.pop(), hosts.as_slice()) {
        (
            Some(HostInfo {
                cluster_issuers,
                ctl_host,
                id,
                js_domain,
                labels,
                lattice_prefix,
                prov_rpc_host,
                rpc_host,
                uptime_human,
                uptime_seconds,
                version,
            }),
            [],
        ) => {
            // TODO: Validate `issuer`
            ensure!(cluster_issuers == Some("TODO".into()));
            ensure!(ctl_host == Some("TODO".into()));
            ensure!(id == host_key.public_key());
            ensure!(js_domain == Some("TODO".into()));
            ensure!(
                labels.as_ref() == Some(&expected_labels),
                r#"invalid labels:
got: {labels:?}
expected: {expected_labels:?}"#
            );
            ensure!(lattice_prefix == Some(TEST_PREFIX.into()));
            ensure!(prov_rpc_host == Some("TODO".into()));
            ensure!(rpc_host == Some("TODO".into()));
            ensure!(uptime_human == Some("TODO".into()));
            ensure!(uptime_seconds >= 0);
            ensure!(version == Some(env!("CARGO_PKG_VERSION").into()));
        }
        (None, []) => bail!("no hosts in the lattice"),
        _ => bail!("more than one host in the lattice"),
    }

    let actor = fs::read(test_actors::RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2_SIGNED)
        .await
        .context("failed to read actor")?;
    let jwt::Token {
        claims: actor_claims,
        ..
    } = extract_claims(actor)
        .context("failed to extract actor claims")?
        .context("actor claims missing")?;
    let actor_url =
        Url::from_file_path(test_actors::RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2_SIGNED)
            .expect("failed to construct actor ref");
    let mut ack = ctl_client
        .perform_actor_auction(actor_url.as_str(), HashMap::default())
        .await
        .map_err(|e| anyhow!(e).context("failed to perform actor auction"))?;
    match (ack.pop(), ack.as_slice()) {
        (
            Some(ActorAuctionAck {
                actor_ref,
                host_id,
                constraints,
            }),
            [],
        ) => {
            ensure!(host_id == host_key.public_key());
            ensure!(actor_ref == actor_url.as_str());
            ensure!(constraints.is_empty());
        }
        (None, []) => bail!("no actor auction ack received"),
        _ => bail!("more than one actor auction ack received"),
    }

    let CtlOperationAck { accepted, error } = ctl_client
        .start_actor(&host_key.public_key(), actor_url.as_str(), 1, None)
        .await
        .map_err(|e| anyhow!(e).context("failed to start actor"))?;
    ensure!(error == "");
    ensure!(accepted);

    let httpserver_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPSERVER_SUBJECT)
        .context("failed to parse `rust-httpserver` provider key")?;
    let httpserver_provider_url = Url::from_file_path(test_providers::RUST_HTTPSERVER)
        .expect("failed to construct provider ref");

    let kvredis_provider_key = KeyPair::from_seed(test_providers::RUST_KVREDIS_SUBJECT)
        .context("failed to parse `rust-kvredis` provider key")?;
    let kvredis_provider_url = Url::from_file_path(test_providers::RUST_KVREDIS)
        .expect("failed to construct provider ref");

    let nats_provider_key = KeyPair::from_seed(test_providers::RUST_NATS_SUBJECT)
        .context("failed to parse `rust-nats` provider key")?;
    let nats_provider_url =
        Url::from_file_path(test_providers::RUST_NATS).expect("failed to construct provider ref");

    let mut ack = ctl_client
        .perform_provider_auction(
            httpserver_provider_url.as_str(),
            "default",
            HashMap::default(),
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to perform provider auction"))?;
    match (ack.pop(), ack.as_slice()) {
        (
            Some(ProviderAuctionAck {
                provider_ref,
                host_id,
                link_name,
            }),
            [],
        ) => {
            // TODO: Validate `constraints`
            ensure!(host_id == host_key.public_key());
            ensure!(provider_ref == httpserver_provider_url.as_str());
            ensure!(link_name == "default");
        }
        (None, []) => bail!("no provider auction ack received"),
        _ => bail!("more than one provider auction ack received"),
    }

    let http_port = free_port().await?;

    // NOTE: Links are advertised before the provider is started to prevent race condition, which
    // occurs if link is established after the providers starts, but before it subscribes to NATS
    // topics
    try_join!(
        assert_advertise_link(
            &ctl_client,
            &actor_claims,
            &httpserver_provider_key,
            "wasmcloud:httpserver",
            HashMap::from([(
                "config_json".into(),
                format!(r#"{{"address":"[{}]:{http_port}"}}"#, Ipv6Addr::UNSPECIFIED)
            )]),
        ),
        assert_advertise_link(
            &ctl_client,
            &actor_claims,
            &kvredis_provider_key,
            "wasmcloud:keyvalue",
            HashMap::from([("URL".into(), format!("{redis_url}"))]),
        ),
        assert_advertise_link(
            &ctl_client,
            &actor_claims,
            &nats_provider_key,
            "wasmcloud:messaging",
            HashMap::from([(
                "config_json".into(),
                format!(r#"{{"cluster_uris":["{ctl_nats_url}"]}}"#)
            )]),
        )
    )
    .context("failed to advertise links")?;

    try_join!(
        assert_start_provider(
            &ctl_client,
            &nats_client,
            TEST_PREFIX,
            &host_key,
            &httpserver_provider_key,
            &httpserver_provider_url,
            None,
        ),
        assert_start_provider(
            &ctl_client,
            &nats_client,
            TEST_PREFIX,
            &host_key,
            &kvredis_provider_key,
            &kvredis_provider_url,
            None,
        ),
        assert_start_provider(
            &ctl_client,
            &nats_client,
            TEST_PREFIX,
            &host_key,
            &nats_provider_key,
            &nats_provider_url,
            None,
        )
    )
    .context("failed to start providers")?;

    let HostInventory {
        mut actors,
        host_id,
        labels,
        mut providers,
        issuer,
        friendly_name,
    } = ctl_client
        .get_host_inventory(&host_key.public_key())
        .await
        .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
    ensure!(friendly_name != ""); // TODO: Make sure it's actually friendly?
    ensure!(host_id == host_key.public_key());
    ensure!(issuer == cluster_key.public_key());
    ensure!(
        labels == expected_labels,
        r#"invalid labels:
got: {labels:?}
expected: {expected_labels:?}"#
    );
    match (actors.pop(), actors.as_slice()) {
        (
            Some(ActorDescription {
                id,
                image_ref,
                mut instances,
                name,
            }),
            [],
        ) => {
            // TODO: Validate `constraints`
            ensure!(id == actor_claims.subject);
            let jwt::Actor {
                name: expected_name,
                rev: expected_revision,
                ..
            } = actor_claims
                .metadata
                .as_ref()
                .context("missing actor metadata")?;
            ensure!(image_ref == Some(actor_url.to_string()));
            ensure!(
                name == *expected_name,
                r#"invalid name:
got: {name:?}
expected: {expected_name:?}"#
            );
            let ActorInstance {
                annotations,
                instance_id,
                revision,
            } = instances.pop().context("no actor instances found")?;
            ensure!(instances.is_empty(), "more than one actor instance found");
            ensure!(annotations == None);
            ensure!(Uuid::parse_str(&instance_id).is_ok());
            ensure!(revision == expected_revision.unwrap_or_default());
        }
        (None, []) => bail!("no actor found"),
        _ => bail!("more than one actor found"),
    }
    providers.sort_unstable_by(|a, b| b.name.cmp(&a.name));
    match (
        providers.pop(),
        providers.pop(),
        providers.pop(),
        providers.as_slice(),
    ) {
        (Some(httpserver), Some(kvredis), Some(nats), []) => {
            // TODO: Validate `constraints`
            ensure!(httpserver.annotations == None);
            ensure!(httpserver.id == httpserver_provider_key.public_key());
            ensure!(httpserver.image_ref == Some(httpserver_provider_url.to_string()));
            ensure!(httpserver.contract_id == "wasmcloud:httpserver");
            ensure!(httpserver.link_name == "default");
            ensure!(httpserver.name == Some("wasmcloud-provider-httpserver".into()),);
            ensure!(httpserver.revision == 0);

            // TODO: Validate `constraints`
            ensure!(kvredis.annotations == None);
            ensure!(kvredis.id == kvredis_provider_key.public_key());
            ensure!(kvredis.image_ref == Some(kvredis_provider_url.to_string()));
            ensure!(kvredis.contract_id == "wasmcloud:keyvalue");
            ensure!(kvredis.link_name == "default");
            ensure!(kvredis.name == Some("wasmcloud-provider-kvredis".into()),);
            ensure!(kvredis.revision == 0);

            // TODO: Validate `constraints`
            ensure!(nats.annotations == None);
            ensure!(nats.id == nats_provider_key.public_key());
            ensure!(nats.image_ref == Some(nats_provider_url.to_string()));
            ensure!(nats.contract_id == "wasmcloud:messaging");
            ensure!(nats.link_name == "default");
            ensure!(nats.name == Some("wasmcloud-provider-nats".into()),);
            ensure!(nats.revision == 0);
        }
        _ => bail!("invalid provider count"),
    }

    let (mut nats_publish_sub, mut nats_request_sub, mut nats_request_multi_sub) = try_join!(
        nats_client.subscribe("test-messaging-publish".into()),
        nats_client.subscribe("test-messaging-request".into()),
        nats_client.subscribe("test-messaging-request-multi".into()),
    )
    .context("failed to subscribe to NATS topics")?;

    redis_client
        .req_command(&redis::Cmd::set("foo", "bar"))
        .context("failed to set `foo` key in Redis")?;

    let nats_requests = spawn(async move {
        let res = nats_request_sub
            .next()
            .await
            .context("failed to receive NATS response to `request`")?;
        ensure!(res.payload == "foo");
        let reply = res.reply.context("no reply set on `request`")?;
        nats_client
            .publish(reply, "bar".into())
            .await
            .context("failed to publish response to `request`")?;

        let res = nats_request_multi_sub
            .next()
            .await
            .context("failed to receive NATS response to `request_multi`")?;
        ensure!(res.payload == "foo");
        let reply = res.reply.context("no reply on set `request_multi`")?;
        nats_client
            .publish(reply, "bar".into())
            .await
            .context("failed to publish response to `request_multi`")?;
        Ok(())
    });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")?;
    let http_res = http_client
        .post(format!("http://localhost:{http_port}"))
        .body(r#"{"min":42,"max":4242}"#)
        .send()
        .await
        .context("failed to connect to server")?
        .text()
        .await
        .context("failed to get response text")?;

    // TODO: Instead of duplication here, reuse the same struct used in `wasmcloud-runtime` tests
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    // NOTE: If values are truly random, we have nothing to assert for some of these fields
    struct Response {
        #[allow(dead_code)]
        get_random_bytes: [u8; 8],
        #[allow(dead_code)]
        get_random_u64: u64,
        guid: String,
        random_in_range: u32,
        #[allow(dead_code)]
        random_32: u32,
    }
    let Response {
        get_random_bytes: _,
        get_random_u64: _,
        guid,
        random_32: _,
        random_in_range,
    } = serde_json::from_str(&http_res).context("failed to decode body as JSON")?;
    ensure!(Uuid::from_str(&guid).is_ok());
    ensure!(
        (42..=4242).contains(&random_in_range),
        "{random_in_range} should have been within range from 42 to 4242 inclusive"
    );
    let nats_res = nats_publish_sub
        .next()
        .await
        .context("failed to receive NATS response")?;
    ensure!(nats_res.payload == http_res);
    ensure!(nats_res.reply.as_deref() == Some("noreply"));

    nats_requests
        .await
        .context("failed to await NATS request task")?
        .context("failed to handle NATS requests")?;

    let redis_keys = redis_client
        .req_command(&redis::Cmd::keys("*"))
        .context("failed to list keys in Redis")?;
    let expected_redis_keys = redis::Value::Bulk(vec![redis::Value::Data(b"result".to_vec())]);
    ensure!(
        redis_keys == expected_redis_keys,
        r#"invalid keys in Redis:
got: {redis_keys:?}
expected: {expected_redis_keys:?}"#
    );

    let redis_res = redis_client
        .req_command(&redis::Cmd::get("result"))
        .context("failed to get `result` key in Redis")?;
    ensure!(redis_res == redis::Value::Data(http_res.into()));

    try_join!(
        assert_remove_link(&ctl_client, &actor_claims, "wasmcloud:messaging"),
        assert_remove_link(&ctl_client, &actor_claims, "wasmcloud:keyvalue"),
        assert_remove_link(&ctl_client, &actor_claims, "wasmcloud:httpserver"),
    )
    .context("failed to remove links")?;

    let CtlOperationAck { accepted, error } = ctl_client
        .stop_host(&host_key.public_key(), None)
        .await
        .map_err(|e| anyhow!(e).context("failed to stop host"))?;
    ensure!(error == "");
    ensure!(accepted);

    let _ = host.stopped().await;
    shutdown.await.context("failed to shutdown host")?;

    stop_nats_tx.send(()).expect("failed to stop NATS");
    let nats_status = nats_server
        .await
        .context("failed to wait for NATS to exit")??;
    ensure!(nats_status.code().is_none());

    stop_redis_tx.send(()).expect("failed to stop Redis");
    let redis_status = redis_server
        .await
        .context("failed to wait for Redis to exit")??;
    ensure!(redis_status.code().is_none());

    Ok(())
}

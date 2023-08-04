use std::collections::HashMap;
use std::env;
use std::env::consts::{ARCH, FAMILY, OS};
use std::net::Ipv6Addr;
use std::pin::pin;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context};
use nkeys::KeyPair;
use serde::Deserialize;
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::time::interval;
use tokio::{fs, select, spawn};
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt;
use tracing::warn;
use tracing_subscriber::prelude::*;
use url::Url;
use uuid::Uuid;
use wascap::jwt;
use wascap::wasm::extract_claims;
use wasmcloud_control_interface::{
    ActorAuctionAck, ActorDescription, ActorInstance, ClientBuilder, CtlOperationAck, Host,
    HostInventory, ProviderAuctionAck,
};
use wasmcloud_host::wasmbus::{Lattice, LatticeConfig};

async fn free_port() -> anyhow::Result<u16> {
    let lis = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
        .await
        .context("failed to start TCP listener")?
        .local_addr()
        .context("failed to query listener local address")?;
    Ok(lis.port())
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
    let nats_url =
        Url::parse(&format!("nats://localhost:{nats_port}")).context("failed to parse NATS URL")?;

    let dir = tempdir().context("failed to create temporary directory")?;
    let mut nats = Command::new(
        env::var("WASMCLOUD_NATS")
            .as_ref()
            .map(String::as_str)
            .unwrap_or("nats-server"),
    )
    .args(["-js", "-V", "-T=false", "-p", &nats_port.to_string(), "-sd"])
    .arg(dir.path())
    .kill_on_drop(true)
    .spawn()
    .context("failed to spawn NATS")?;

    let (stop_tx, stop_rx) = oneshot::channel();
    let nats = spawn(async move {
        select!(
            res = stop_rx => {
                res.context("failed to wait for shutdown")?;
                nats.kill().await.context("failed to kill NATS")?;
                nats.wait().await
            }
            status = nats.wait() => {
                status
            }
        )
        .context("failed to wait for NATS")
    });

    let client = async_nats::connect_with_options(
        nats_url.as_str(),
        async_nats::ConnectOptions::new().retry_on_initial_connect(),
    )
    .await
    .context("failed to connect to NATS")?;
    let client = ClientBuilder::new(client)
        // TODO: Remove rpc_timeout in https://github.com/wasmCloud/wasmCloud/issues/367
        .rpc_timeout(Duration::from_secs(20))
        .build()
        .await
        .map_err(|e| anyhow!(e).context("failed to build client"))?;

    let cluster_key = KeyPair::new_cluster();
    let host_key = KeyPair::new_server();

    env::set_var("HOST_PATH", "test-path");
    let expected_labels = HashMap::from([
        ("hostcore.arch".into(), ARCH.into()),
        ("hostcore.os".into(), OS.into()),
        ("hostcore.osfamily".into(), FAMILY.into()),
        ("path".into(), "test-path".into()),
    ]);
    let (lattice, shutdown) = Lattice::new(LatticeConfig {
        url: nats_url.clone(),
        cluster_seed: Some(cluster_key.seed().unwrap()),
        host_seed: Some(host_key.seed().unwrap()),
    })
    .await
    .context("failed to initialize `wasmbus` lattice")?;

    let mut hosts = client
        .get_hosts()
        .await
        .map_err(|e| anyhow!(e).context("failed to get hosts"))?;
    match (hosts.pop(), hosts.as_slice()) {
        (
            Some(Host {
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
                "invalid labels:\ngot: {labels:?}\nexpected: {expected_labels:?}"
            );
            ensure!(lattice_prefix == Some("default".into()));
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
    let mut ack = client
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

    let CtlOperationAck { accepted, error } = client
        .start_actor(&host_key.public_key(), actor_url.as_str(), 1, None)
        .await
        .map_err(|e| anyhow!(e).context("failed to start actor"))?;
    ensure!(error == "");
    ensure!(accepted);

    let httpserver_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPSERVER_SUBJECT)
        .context("failed to parse `rust-httpserver` provider key")?;
    let httpserver_provider_url = Url::from_file_path(test_providers::RUST_HTTPSERVER)
        .expect("failed to construct provider ref");

    let nats_provider_key = KeyPair::from_seed(test_providers::RUST_NATS_SUBJECT)
        .context("failed to parse `rust-nats` provider key")?;
    let nats_provider_url =
        Url::from_file_path(test_providers::RUST_NATS).expect("failed to construct provider ref");

    let mut ack = client
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
    let CtlOperationAck { accepted, error } = client
        .advertise_link(
            &actor_claims.subject,
            &httpserver_provider_key.public_key(),
            "wasmcloud:httpserver",
            "default",
            HashMap::from([(
                "config_json".into(),
                format!(r#"{{"address":"[{}]:{http_port}"}}"#, Ipv6Addr::UNSPECIFIED),
            )]),
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to advertise link"))?;
    ensure!(error == "");
    ensure!(accepted);

    let CtlOperationAck { accepted, error } = client
        .advertise_link(
            &actor_claims.subject,
            &nats_provider_key.public_key(),
            "wasmcloud:messaging",
            "default",
            HashMap::from([(
                "config_json".into(),
                format!(r#"{{"cluster_uris":"{nats_url}"}}"#),
            )]),
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to advertise link"))?;
    ensure!(error == "");
    ensure!(accepted);

    let CtlOperationAck { accepted, error } = client
        .start_provider(
            &host_key.public_key(),
            httpserver_provider_url.as_str(),
            None,
            None,
            None,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to start provider"))?;
    ensure!(error == "");
    ensure!(accepted);

    let CtlOperationAck { accepted, error } = client
        .start_provider("", nats_provider_url.as_str(), None, None, None)
        .await
        .map_err(|e| anyhow!(e).context("failed to start provider"))?;
    ensure!(error == "");
    ensure!(accepted);

    let HostInventory {
        mut actors,
        host_id,
        labels,
        mut providers,
        issuer,
        friendly_name,
    } = client
        .get_host_inventory(&host_key.public_key())
        .await
        .map_err(|e| anyhow!(e).context("failed to start provider"))?;
    ensure!(friendly_name != ""); // TODO: Make sure it's actually friendly?
    ensure!(host_id == host_key.public_key());
    ensure!(issuer == cluster_key.public_key());
    ensure!(
        labels == expected_labels,
        "invalid labels:\ngot: {labels:?}\nexpected: {expected_labels:?}"
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
            } = actor_claims.metadata.context("missing actor metadata")?;
            ensure!(image_ref == Some(actor_url.to_string()));
            ensure!(
                name == expected_name,
                "invalid name:\ngot: {name:?}\nexpected: {expected_name:?}"
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
    match (providers.pop(), providers.pop(), providers.as_slice()) {
        (Some(httpserver), Some(nats), []) => {
            // TODO: Validate `constraints`
            ensure!(httpserver.annotations == None);
            ensure!(httpserver.id == httpserver_provider_key.public_key());
            ensure!(httpserver.image_ref == Some(httpserver_provider_url.to_string()));
            ensure!(httpserver.contract_id == "wasmcloud:httpserver");
            ensure!(httpserver.link_name == "default");
            ensure!(httpserver.name == Some("wasmcloud-provider-httpserver".into()),);
            ensure!(httpserver.revision == 0);

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

    let res = pin!(IntervalStream::new(interval(Duration::from_secs(1)))
        .take(10)
        .then(|_| async {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .connect_timeout(Duration::from_secs(20))
                .build()
                .context("failed to build HTTP client")?
                .post(format!("http://localhost:{http_port}"))
                .body(r#"{"min":42,"max":4242}"#)
                .send()
                .await?
                .text()
                .await
                .context("failed to get response text")
        })
        .filter_map(|res| {
            match res {
                Err(error) => {
                    warn!(?error, "failed to connect to server");
                    None
                }
                Ok(res) => Some(res),
            }
        }))
    .next()
    .await
    .context("failed to connect to server")?;

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
    } = serde_json::from_str(&res).context("failed to decode body as JSON")?;
    ensure!(Uuid::from_str(&guid).is_ok());
    ensure!(
        (42..=4242).contains(&random_in_range),
        "{random_in_range} should have been within range from 42 to 4242 inclusive"
    );

    let CtlOperationAck { accepted, error } = client
        .remove_link(&actor_claims.subject, "wasmcloud:messaging", "default")
        .await
        .map_err(|e| anyhow!(e).context("failed to remove link"))?;
    ensure!(error == "");
    ensure!(accepted);

    let CtlOperationAck { accepted, error } = client
        .remove_link(&actor_claims.subject, "wasmcloud:httpserver", "default")
        .await
        .map_err(|e| anyhow!(e).context("failed to remove link"))?;
    ensure!(error == "");
    ensure!(accepted);

    let CtlOperationAck { accepted, error } = client
        .stop_host(&host_key.public_key(), None)
        .await
        .map_err(|e| anyhow!(e).context("failed to stop host"))?;
    ensure!(error == "");
    ensure!(accepted);

    let _ = lattice.stopped().await;
    shutdown.await.context("failed to shutdown lattice")?;

    stop_tx.send(()).expect("failed to stop NATS");

    let status = nats.await.context("failed to wait for NATS to exit")??;
    ensure!(status.code().is_none());

    Ok(())
}

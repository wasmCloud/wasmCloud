use crate::common::{
    await_actor_count, await_provider_count, par_from_file, HTTPSRV_OCI, KVCOUNTER_OCI, NATS_OCI,
    REDIS_OCI,
};
use ::wasmcloud_control_interface::Client;
use actix_rt::time::delay_for;
use std::collections::HashMap;
use wasmcloud_actor_http_server::{deserialize, serialize};

use std::time::Duration;

use wascap::prelude::KeyPair;
use wasmcloud_host::{Actor, HostBuilder};
use wasmcloud_host::{NativeCapability, Result};

// NOTE: this test does verify a number of error and edge cases, so when it is
// running -properly- you will see warnings and errors in the output log
pub(crate) async fn basics() -> Result<()> {
    // Ensure that we're not accidentally using the replication feature on KV cache
    ::std::env::remove_var("KVCACHE_NATS_URL");

    let nc = nats::asynk::connect("0.0.0.0:4222").await?;
    let nc3 = nats::asynk::connect("0.0.0.0:4222").await?;
    let h = HostBuilder::new()
        .with_namespace("controlbasics")
        .with_rpc_client(nc3)
        .with_control_client(nc)
        .oci_allow_latest()
        .with_label("testing", "test-one")
        .build();

    h.start().await?;
    let hid = h.id();
    let nc2 = nats::asynk::connect("0.0.0.0:4222").await?;

    let ctl_client = Client::new(
        nc2,
        Some("controlbasics".to_string()),
        Duration::from_secs(20),
    );

    // Cannot stop a non-existent actor
    assert!(ctl_client
        .stop_actor(&hid, KVCOUNTER_OCI)
        .await?
        .failure
        .is_some());
    // Cannot stop a non-existent provider
    assert!(ctl_client
        .stop_provider(&hid, "fooref", "default", "wasmcloud:testing")
        .await?
        .failure
        .is_some());

    let a_ack = ctl_client.start_actor(&hid, KVCOUNTER_OCI).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 20).await?;
    println!("Received ACK from host {}", a_ack.host_id);

    let claims = ctl_client.get_claims().await?;
    assert_eq!(1, claims.claims.len());
    assert!(a_ack.failure.is_none());

    let a_ack2 = ctl_client.start_actor(&hid, KVCOUNTER_OCI).await?;
    assert!(a_ack2.failure.is_some()); // cannot start the same actor twice
    assert_eq!(
        a_ack2.failure.unwrap(),
        format!(
            "Actor with image ref '{}' is already running on this host",
            KVCOUNTER_OCI
        )
    );

    let stop_ack = ctl_client.stop_actor(&hid, KVCOUNTER_OCI).await?;
    assert!(stop_ack.failure.is_none());
    await_actor_count(&h, 0, Duration::from_millis(50), 20).await?;

    let _ = ctl_client.start_actor(&hid, KVCOUNTER_OCI).await?;

    let redis_ack = ctl_client.start_provider(&hid, REDIS_OCI, None).await?;
    await_provider_count(&h, 3, Duration::from_millis(50), 20).await?;
    println!("Redis {:?} started", redis_ack);
    delay_for(Duration::from_millis(500)).await;

    // Stop and re-start a provider
    assert!(ctl_client
        .stop_provider(&hid, REDIS_OCI, "default", "wasmcloud:keyvalue")
        .await?
        .failure
        .is_none());
    await_provider_count(&h, 2, Duration::from_millis(50), 20).await?;
    delay_for(Duration::from_secs(1)).await;
    assert!(ctl_client
        .start_provider(&hid, REDIS_OCI, None)
        .await?
        .failure
        .is_none());
    await_provider_count(&h, 3, Duration::from_millis(50), 20).await?;
    delay_for(Duration::from_secs(1)).await;

    let nats_ack = ctl_client.start_provider(&hid, NATS_OCI, None).await?;
    await_provider_count(&h, 4, Duration::from_millis(50), 200).await?;
    println!("NATS {:?} started", nats_ack);

    let http_ack = ctl_client.start_provider(&hid, HTTPSRV_OCI, None).await?;
    await_provider_count(&h, 5, Duration::from_millis(50), 10).await?;
    println!("HTTP Server {:?} started", http_ack);

    let http_ack2 = ctl_client.start_provider(&hid, HTTPSRV_OCI, None).await?;
    assert!(http_ack2.failure.is_some());
    assert_eq!(
        http_ack2.failure.unwrap(),
        format!(
            "Provider with image ref '{}' is already running on this host.",
            HTTPSRV_OCI
        )
    );

    let hosts = ctl_client.get_hosts(Duration::from_secs(1)).await?;
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].id, hid);

    let inv = ctl_client.get_host_inventory(&hosts[0].id).await?;
    println!("Got host inventory: {:?}", inv);
    //assert_eq!(3, inv.providers.len());
    assert_eq!(1, inv.actors.len());
    assert_eq!(inv.actors[0].image_ref, Some(KVCOUNTER_OCI.to_string()));
    assert_eq!(4, inv.labels.len()); // each host gets 3 built-in labels
    assert_eq!(inv.host_id, hosts[0].id);
    h.stop().await;
    Ok(())
}

pub(crate) async fn calltest() -> Result<()> {
    // Ensure that we're not accidentally using the replication feature on KV cache
    ::std::env::remove_var("KVCACHE_NATS_URL");

    let nc = nats::asynk::connect("0.0.0.0:4222").await?;
    let nc3 = nats::asynk::connect("0.0.0.0:4222").await?;
    let h = HostBuilder::new()
        .with_namespace("calltest")
        .with_control_client(nc)
        .with_rpc_client(nc3)
        .build();

    h.start().await?;
    let a = Actor::from_file("./tests/modules/echo.wasm")?;
    let a_id = a.public_key();
    h.start_actor(a).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 20).await?;
    delay_for(Duration::from_millis(600)).await;

    let nc2 = nats::asynk::connect("0.0.0.0:4222").await?;

    let ctl_client = Client::new(nc2, Some("calltest".to_string()), Duration::from_secs(20));

    let req = wasmcloud_actor_http_server::Request {
        header: HashMap::new(),
        method: "GET".to_string(),
        path: "".to_string(),
        query_string: "".to_string(),
        body: b"NARF".to_vec(),
    };
    let inv_r = ctl_client
        .call_actor(&a_id, "HandleRequest", &serialize(&req)?)
        .await?;
    let http_r: wasmcloud_actor_http_server::Response = deserialize(&inv_r.msg)?;

    assert_eq!(inv_r.error, None);
    assert_eq!(
        std::str::from_utf8(&http_r.body)?,
        r#"{"method":"GET","path":"","query_string":"","headers":{},"body":[78,65,82,70]}"#
    );
    assert_eq!(http_r.status, "OK".to_string());
    assert_eq!(http_r.status_code, 200);
    h.stop().await;
    delay_for(Duration::from_millis(900)).await;

    ctl_client.stop_actor(&h.id(), &a_id).await?;
    delay_for(Duration::from_millis(300)).await;
    let inv_r = ctl_client
        .call_actor(&a_id, "HandleRequest", &serialize(&req)?)
        .await;

    println!("{:?}", inv_r);
    // we should not be able to invoke an actor that we stopped
    assert!(inv_r.is_err());

    Ok(())
}

pub(crate) async fn auctions() -> Result<()> {
    // Auctions tests require that the hosts are at the very least
    // sharing the same lattice data.

    // Set the default kvcache provider to enable NATS-based replication
    // by supplying a NATS URL.
    ::std::env::set_var("KVCACHE_NATS_URL", "0.0.0.0:4222");

    let nc = nats::asynk::connect("0.0.0.0:4222").await?;
    let h = HostBuilder::new()
        .with_namespace("auctions")
        .with_control_client(nc)
        .oci_allow_latest()
        .with_label("kv-friendly", "yes")
        .with_label("web-friendly", "no")
        .build();

    h.start().await?;
    let hid = h.id();
    let nc2 = nats::asynk::connect("0.0.0.0:4222").await?;
    let nc3 = nats::asynk::connect("0.0.0.0:4222").await?;

    let ctl_client = Client::new(nc2, Some("auctions".to_string()), Duration::from_secs(20));

    let h2 = HostBuilder::new()
        .with_namespace("auctions")
        .with_control_client(nc3)
        .oci_allow_latest()
        .with_label("web-friendly", "yes")
        .build();
    h2.start().await?;
    let hid2 = h2.id();

    delay_for(Duration::from_secs(2)).await;

    // auction with no requirements
    let kvack = ctl_client
        .perform_actor_auction(KVCOUNTER_OCI, HashMap::new(), Duration::from_secs(5))
        .await?;
    assert_eq!(2, kvack.len());

    // auction the KV counter with a constraint
    let kvack = ctl_client
        .perform_actor_auction(KVCOUNTER_OCI, kvrequirements(), Duration::from_secs(5))
        .await?;
    assert_eq!(1, kvack.len());
    assert_eq!(kvack[0].host_id, hid);

    // start it and re-attempt an auction
    let _ = ctl_client.start_actor(&hid, KVCOUNTER_OCI).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 20).await?;
    delay_for(Duration::from_secs(1)).await;

    let kvack = ctl_client
        .perform_actor_auction(KVCOUNTER_OCI, kvrequirements(), Duration::from_secs(5))
        .await?;
    // Should be no viable candidates now
    assert_eq!(0, kvack.len());

    // find a place for the web server
    let httpack = ctl_client
        .perform_provider_auction(
            HTTPSRV_OCI,
            "default",
            webrequirements(),
            Duration::from_secs(2),
        )
        .await?;
    assert_eq!(1, httpack.len());
    assert_eq!(httpack[0].host_id, hid2);

    // start web server on host 2
    let _http_ack = ctl_client
        .start_provider(&httpack[0].host_id, HTTPSRV_OCI, None)
        .await?;
    await_provider_count(&h2, 3, Duration::from_millis(50), 10).await?;
    delay_for(Duration::from_millis(500)).await;

    // should be no candidates now
    let httpack = ctl_client
        .perform_provider_auction(
            HTTPSRV_OCI,
            "default",
            webrequirements(),
            Duration::from_secs(1),
        )
        .await?;
    assert_eq!(0, httpack.len());
    h.stop().await;
    h2.stop().await;
    delay_for(Duration::from_millis(300)).await;
    Ok(())
}

fn kvrequirements() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("kv-friendly".to_string(), "yes".to_string());
    hm
}

fn webrequirements() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("web-friendly".to_string(), "yes".to_string());
    hm
}

fn embed_revision(source: &[u8], kp: &KeyPair, rev: i32, subject: &str, issuer: &str) -> Vec<u8> {
    let claims = wascap::jwt::Claims::<wascap::jwt::Actor>::new(
        "Testy McTestFace".to_string(),
        issuer.to_string(),
        subject.to_string(),
        Some(vec!["test:testo".to_string()]),
        None,
        false,
        Some(rev),
        None,
        None,
    );
    wascap::wasm::embed_claims(source, &claims, kp).unwrap()
}

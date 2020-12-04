use crate::common::{
    await_actor_count, await_provider_count, par_from_file, HTTPSRV_OCI, KVCOUNTER_OCI, NATS_OCI,
    REDIS_OCI,
};
use ::control_interface::Client;
use actix_rt::time::delay_for;
use std::collections::HashMap;
use std::thread;
use std::time::Duration;
use wascc_redis::RedisKVProvider;
use wasmcloud_host::Result;
use wasmcloud_host::{HostBuilder, NativeCapability};

pub(crate) async fn basics() -> Result<()> {
    let nc = nats::asynk::connect("0.0.0.0:4222").await?;
    let h = HostBuilder::new()
        .with_namespace("controlbasics")
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
        .stop_provider(&hid, "fooref", "default", "wascc:testing")
        .await?
        .failure
        .is_some());

    let a_ack = ctl_client.start_actor(&hid, KVCOUNTER_OCI).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 20).await?;
    println!("Actor {} started on host {}", a_ack.actor_id, a_ack.host_id);

    let claims = ctl_client.get_claims().await?;
    assert_eq!(1, claims.claims.len());
    assert_eq!(a_ack.actor_id, claims.claims[0].values["sub"]);
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
    await_provider_count(&h, 2, Duration::from_millis(50), 20).await?;
    println!("Redis {:?} started", redis_ack);

    // Stop and re-start a provider
    assert!(ctl_client
        .stop_provider(&hid, REDIS_OCI, "default", "wascc:key_value")
        .await?
        .failure
        .is_none());
    await_provider_count(&h, 1, Duration::from_millis(50), 20).await?;
    assert!(ctl_client
        .start_provider(&hid, REDIS_OCI, None)
        .await?
        .failure
        .is_none());
    await_provider_count(&h, 2, Duration::from_millis(50), 20).await?;

    let nats_ack = ctl_client.start_provider(&hid, NATS_OCI, None).await?;
    await_provider_count(&h, 3, Duration::from_millis(10), 200).await?;
    println!("NATS {:?} started", nats_ack);

    /* let redis_claims = {
        let arc = par_from_file("./tests/modules/libwascc_redis.par.gz")?;
        arc.claims().unwrap()
    };
    let redis = RedisKVProvider::new();
    let redcap = NativeCapability::from_instance(redis, None, redis_claims)?;
    h.start_native_capability(redcap).await?;
    await_provider_count(&h, 2, Duration::from_millis(50), 10)
        .await?; */

    let http_ack = ctl_client.start_provider(&hid, HTTPSRV_OCI, None).await?;
    await_provider_count(&h, 4, Duration::from_millis(50), 10).await?;
    println!("HTTP Server {:?} started", http_ack);

    let http_ack2 = ctl_client.start_provider(&hid, HTTPSRV_OCI, None).await?;
    assert!(http_ack2.failure.is_some());
    assert_eq!(
        http_ack2.failure.unwrap(),
        "Provider with image ref 'wascc.azurecr.io/httpsrv:v1' is already running on this host."
    );

    let hosts = ctl_client.get_hosts(Duration::from_millis(500)).await?;
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].id, hid);

    let inv = ctl_client.get_host_inventory(&hosts[0].id).await?;
    //assert_eq!(3, inv.providers.len());
    assert_eq!(1, inv.actors.len());
    assert_eq!(inv.actors[0].image_ref, Some(KVCOUNTER_OCI.to_string()));
    assert_eq!(4, inv.labels.len()); // each host gets 3 built-in labels
    assert_eq!(inv.host_id, hosts[0].id);
    assert!(inv
        .providers
        .iter()
        .find(|p| p.image_ref == Some(HTTPSRV_OCI.to_string()) && p.id == http_ack.provider_id)
        .is_some());

    delay_for(Duration::from_secs(1)).await;
    h.stop().await;
    delay_for(Duration::from_secs(1)).await;

    //h.stop().await;

    Ok(())
}

pub(crate) async fn auctions() -> Result<()> {
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

    // auction with no requirements
    let kvack = ctl_client
        .perform_actor_auction(KVCOUNTER_OCI, HashMap::new(), Duration::from_millis(200))
        .await?;
    assert_eq!(2, kvack.len());

    // auction the KV counter with a constraint
    let kvack = ctl_client
        .perform_actor_auction(KVCOUNTER_OCI, kvrequirements(), Duration::from_millis(200))
        .await?;
    assert_eq!(1, kvack.len());
    assert_eq!(kvack[0].host_id, hid);

    // start it and re-attempt an auction
    let _ = ctl_client.start_actor(&hid, KVCOUNTER_OCI).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 20).await?;

    let kvack = ctl_client
        .perform_actor_auction(KVCOUNTER_OCI, kvrequirements(), Duration::from_millis(500))
        .await?;
    // Should be no viable candidates now
    assert_eq!(0, kvack.len());

    // find a place for the web server
    let httpack = ctl_client
        .perform_provider_auction(
            HTTPSRV_OCI,
            "default",
            webrequirements(),
            Duration::from_millis(200),
        )
        .await?;
    assert_eq!(1, httpack.len());
    assert_eq!(httpack[0].host_id, hid2);

    // start web server on host 2
    let _http_ack = ctl_client
        .start_provider(&httpack[0].host_id, HTTPSRV_OCI, None)
        .await?;
    await_provider_count(&h2, 2, Duration::from_millis(50), 10).await?;

    // should be no candidates now
    let httpack = ctl_client
        .perform_provider_auction(
            HTTPSRV_OCI,
            "default",
            webrequirements(),
            Duration::from_millis(200),
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

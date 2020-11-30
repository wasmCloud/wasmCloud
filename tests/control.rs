use crate::common::{
    await_actor_count, await_provider_count, par_from_file, HTTPSRV_OCI, NATS_OCI, REDIS_OCI,
};
use ::control_interface::Client;
use actix_rt::time::delay_for;
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
        Duration::from_secs(10),
    );

    let a_ack = ctl_client
        .start_actor(&hid, "wascc.azurecr.io/kvcounter:v1")
        .await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 20).await?;
    println!("Actor {} started on host {}", a_ack.actor_id, a_ack.host_id);

    let claims = ctl_client.get_claims().await?;
    assert_eq!(1, claims.claims.len());
    assert_eq!(a_ack.actor_id, claims.claims[0].values["sub"]);
    assert!(a_ack.failure.is_none());

    let a_ack2 = ctl_client
        .start_actor(&hid, "wascc.azurecr.io/kvcounter:v1")
        .await?;
    assert!(a_ack2.failure.is_some()); // cannot start the same actor twice
    assert_eq!(
        a_ack2.failure.unwrap(),
        "Actor with image ref 'wascc.azurecr.io/kvcounter:v1' is already running on this host"
    );

    /*let redis_ack = ctl_client
        .start_provider(&hid, REDIS_OCI, None)
        .await?;
    await_provider_count(&h, 2, Duration::from_millis(50), 20).await?;
    println!("Redis {:?} started", redis_ack); */

    let nats_ack = ctl_client.start_provider(&hid, NATS_OCI, None).await?;
    await_provider_count(&h, 2, Duration::from_millis(10), 200).await?;
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
    await_provider_count(&h, 3, Duration::from_millis(50), 10).await?;
    println!("HTTP Server {:?} started", http_ack);

    /*let http_ack2 = ctl_client
        .start_provider(&hid, HTTPSRV_OCI, None)
        .await?;
    assert!(http_ack2.failure.is_some());
    assert_eq!(
        http_ack2.failure.unwrap(),
        "Provider with image ref 'wascc.azurecr.io/httpsrv:v1' is already running on this host."
    ); */

    let hosts = ctl_client.get_hosts(Duration::from_millis(500)).await?;
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].id, hid);

    let inv = ctl_client.get_host_inventory(&hosts[0].id).await?;
    //assert_eq!(3, inv.providers.len());
    assert_eq!(1, inv.actors.len());
    assert_eq!(4, inv.labels.len()); // each host gets 3 built-in labels
    assert_eq!(inv.host_id, hosts[0].id);

    println!("{:?}", hosts);

    h.stop().await;
    delay_for(Duration::from_secs(1)).await;

    //h.stop().await;

    Ok(())
}

use crate::common::{await_actor_count, await_provider_count, gen_kvcounter_host, par_from_file};
use crate::generated::http::{deserialize, serialize, Request, Response};
use lattice_rpc_nats::NatsLatticeProvider;
use provider_archive::ProviderArchive;
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Write};
use std::time::Duration;
use wascc_host::Result;
use wascc_host::{Actor, HostBuilder, NativeCapability};

// Start two hosts, A and B. Host A contains an actor
// and host B contains a provider. Set a link via host B's
// API and then invoke the provider's running HTTP endpoint
// to ensure the RPC link between actor and provider works
pub(crate) async fn distributed_echo() -> Result<()> {
    let host_a = HostBuilder::new().build();
    let nc = nats::connect("0.0.0.0:4222")?;
    let nats_rpc = Box::new(NatsLatticeProvider::new(
        Some("distributedecho".to_string()),
        Duration::from_millis(300),
        nc.clone(),
    ));

    host_a.start(Some(nats_rpc), None).await?;
    let echo = Actor::from_file("./tests/modules/echo.wasm")?;
    let actor_id = echo.public_key();
    host_a.start_actor(echo).await?;
    await_actor_count(&host_a, 1, Duration::from_millis(50), 3).await?;

    let host_b = HostBuilder::new().build();
    let nats_rpc_b = Box::new(NatsLatticeProvider::new(
        Some("distributedecho".to_string()),
        Duration::from_millis(300),
        nc.clone(),
    ));
    host_b.start(Some(nats_rpc_b), None).await?;
    let web_port = 7001_u32;
    let arc = par_from_file("./tests/modules/libwascc_httpsrv.par.gz")?;
    let websrv = NativeCapability::from_archive(&arc, None)?;
    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));
    host_b.start_native_capability(websrv).await?;
    // always have to remember that "extras" is in the provider list.
    await_provider_count(&host_b, 2, Duration::from_millis(50), 3).await?;
    host_b
        .set_binding(
            &actor_id,
            "wascc:http_server",
            None,
            arc.claims().unwrap().subject.to_string(),
            webvalues,
        )
        .await?;

    let url = format!("http://localhost:{}/foo/bar", web_port);
    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await?,
     "{\"method\":\"GET\",\"path\":\"/foo/bar\",\"query_string\":\"\",\"headers\":{\"accept\":\"*/*\",\"host\":\"localhost:7001\"},\"body\":[]}");

    host_a.stop().await;
    host_b.stop().await;
    Ok(())
}

// Identical to the previous sample, except that a third (Started but empty) host
// is used to receive the set_link call, ensuring that any link can be set from
// anywhere in the lattice.
pub(crate) async fn link_on_third_host() -> Result<()> {
    let host_a = HostBuilder::new().build();
    let nc = nats::connect("0.0.0.0:4222")?;
    let nats_rpc = Box::new(NatsLatticeProvider::new(
        Some("distributedecho".to_string()),
        Duration::from_millis(300),
        nc.clone(),
    ));

    host_a.start(Some(nats_rpc), None).await?;
    let echo = Actor::from_file("./tests/modules/echo.wasm")?;
    let actor_id = echo.public_key();
    host_a.start_actor(echo).await?;
    await_actor_count(&host_a, 1, Duration::from_millis(50), 3).await?;

    let host_b = HostBuilder::new().build();
    let nats_rpc_b = Box::new(NatsLatticeProvider::new(
        Some("distributedecho".to_string()),
        Duration::from_millis(300),
        nc.clone(),
    ));
    host_b.start(Some(nats_rpc_b), None).await?;
    let web_port = 7002_u32;
    let arc = par_from_file("./tests/modules/libwascc_httpsrv.par.gz")?;
    let websrv = NativeCapability::from_archive(&arc, None)?;
    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));
    host_b.start_native_capability(websrv).await?;
    // always have to remember that "extras" is in the provider list.
    await_provider_count(&host_b, 2, Duration::from_millis(50), 3).await?;

    let host_c = HostBuilder::new().build();
    let nats_rpc_c = Box::new(NatsLatticeProvider::new(
        Some("distributedecho".to_string()),
        Duration::from_millis(300),
        nc.clone(),
    ));
    host_c.start(Some(nats_rpc_c), None).await?;
    host_c
        .set_binding(
            &actor_id,
            "wascc:http_server",
            None,
            arc.claims().unwrap().subject.to_string(),
            webvalues,
        )
        .await?;

    let url = format!("http://localhost:{}/foo/bar", web_port);
    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await?,
               "{\"method\":\"GET\",\"path\":\"/foo/bar\",\"query_string\":\"\",\"headers\":{\"accept\":\"*/*\",\"host\":\"localhost:7002\"},\"body\":[]}");

    host_a.stop().await;
    host_b.stop().await;
    host_c.stop().await;
    Ok(())
}

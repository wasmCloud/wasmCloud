use crate::common::{await_actor_count, await_provider_count, par_from_file, REDIS_OCI};
use provider_archive::ProviderArchive;
use std::collections::HashMap;
use std::time::Duration;
use wasmcloud_host::{Actor, HostBuilder, NativeCapability};
use wasmcloud_host::{Host, Result};

// Start two hosts, A and B. Host A contains an actor
// and host B contains a provider. Set a link via host B's
// API and then invoke the provider's running HTTP endpoint
// to ensure the RPC link between actor and provider works
pub(crate) async fn distributed_echo() -> Result<()> {
    // Set the default kvcache provider to enable NATS-based replication
    // by supplying a NATS URL.
    actix_rt::time::sleep(Duration::from_millis(500)).await;
    ::std::env::set_var("KVCACHE_NATS_URL", "0.0.0.0:4222");

    let web_port = 32400_u32;
    let echo = Actor::from_file("./tests/modules/echo.wasm")?;
    let actor_id = echo.public_key();
    let aid = actor_id.clone();

    let nc = nats::asynk::connect("0.0.0.0:4222").await?;
    let host_a = HostBuilder::new()
        .with_rpc_client(nc)
        .with_namespace("distributedecho")
        .build();

    host_a.start().await?;
    let nc2 = nats::asynk::connect("0.0.0.0:4222").await?;
    let host_b = HostBuilder::new()
        .with_rpc_client(nc2)
        .with_namespace("distributedecho")
        .build();

    host_a.start_actor(echo).await?;
    await_actor_count(&host_a, 1, Duration::from_millis(500), 3).await?;

    let arc = par_from_file("./tests/modules/httpserver.par.gz").unwrap();
    let websrv = NativeCapability::from_instance(
        wasmcloud_httpserver::HttpServerProvider::new(),
        None,
        arc.claims().clone().unwrap(),
    )?;

    // ** NOTE - we should be able to start host B in any order because when the cache client starts
    // it should request a replay of cache events and therefore get the existing claims, links,
    // etc.
    host_b.start().await?;
    actix_rt::time::sleep(Duration::from_millis(500)).await;

    host_b.start_native_capability(websrv).await?;
    // always have to remember that "extras" and kvcache is in the provider list.
    await_provider_count(&host_b, 3, Duration::from_millis(500), 3).await?;
    actix_rt::time::sleep(Duration::from_millis(500)).await;

    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));
    host_b
        .set_link(
            &aid,
            "wasmcloud:httpserver",
            None,
            arc.claims().unwrap().subject.to_string(),
            webvalues,
        )
        .await?;

    actix_rt::time::sleep(Duration::from_millis(500)).await;

    let url = format!("http://localhost:{}/foo/bar", web_port);
    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await?,
     "{\"method\":\"GET\",\"path\":\"/foo/bar\",\"query_string\":\"\",\"headers\":{\"host\":\"localhost:32400\",\"accept\":\"*/*\"},\"body\":[]}");

    host_a.stop().await;
    host_b.stop().await;
    actix_rt::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

// Identical to the previous sample, except that a third (Started but empty) host
// is used to receive the set_link call, ensuring that any link can be set from
// anywhere in the lattice.
pub(crate) async fn link_on_third_host() -> Result<()> {
    // Set the default kvcache provider to enable NATS-based replication
    // by supplying a NATS URL.
    ::std::env::set_var("KVCACHE_NATS_URL", "0.0.0.0:4222");
    const NS: &str = "linkonthirdhost";

    let nc = nats::asynk::connect("0.0.0.0:4222").await?;
    let host_a = HostBuilder::new()
        .with_rpc_client(nc)
        .with_namespace(NS)
        .build();

    host_a.start().await?;

    let nc2 = nats::asynk::connect("0.0.0.0:4222").await?;
    let host_b = HostBuilder::new()
        .with_rpc_client(nc2)
        .with_namespace(NS)
        .build();

    host_b.start().await?;

    let echo = Actor::from_file("./tests/modules/echo.wasm")?;
    let actor_id = echo.public_key();
    host_a.start_actor(echo).await?;
    await_actor_count(&host_a, 1, Duration::from_millis(50), 3).await?;

    let web_port = 7002_u32;
    let arc = par_from_file("./tests/modules/httpserver.par.gz")?;
    let websrv = NativeCapability::from_archive(&arc, None)?;

    host_b.start_native_capability(websrv).await?;
    // always have to remember that "extras" is in the provider list.
    await_provider_count(&host_b, 2, Duration::from_millis(50), 3).await?;

    let nc3 = nats::asynk::connect("0.0.0.0:4222").await?;
    let host_c = HostBuilder::new()
        .with_rpc_client(nc3)
        .with_namespace(NS)
        .build();

    host_c.start().await?;
    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));
    host_c
        .set_link(
            &actor_id,
            "wasmcloud:httpserver",
            None,
            arc.claims().unwrap().subject.to_string(),
            webvalues,
        )
        .await?;

    // let the HTTP server spin up
    actix_rt::time::sleep(Duration::from_millis(150)).await;

    let url = format!("http://localhost:{}/foo/bar", web_port);
    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await?,
               "{\"method\":\"GET\",\"path\":\"/foo/bar\",\"query_string\":\"\",\"headers\":{\"host\":\"localhost:7002\",\"accept\":\"*/*\"},\"body\":[]}");

    host_a.stop().await;
    host_b.stop().await;
    host_c.stop().await;
    Ok(())
}

// Identical to the "link on third host" test, but the means of storing/retrieving lattice cache
// values is the Redis provider instead of the default NATS-based replication provider.
// Another reason these tests need to be single-threaded - this test purges the redis database
// before and after
pub(crate) async fn redis_kvcache() -> Result<()> {
    // Configure the -redis- cache provider
    ::std::env::set_var("KVCACHE_URL", "redis://127.0.0.1:6379/1"); // use alternate "2nd" db

    let client = redis::Client::open("redis://127.0.0.1/1")?;
    let mut con = client.get_connection()?;
    redis::cmd("FLUSHDB").execute(&mut con);

    const NS: &str = "rediskvcache";

    let nc = nats::asynk::connect("0.0.0.0:4222").await?;
    let host_a = HostBuilder::new()
        .with_lattice_cache_provider(REDIS_OCI)
        .with_rpc_client(nc)
        .with_namespace(NS)
        .build();

    host_a.start().await?;

    let nc2 = nats::asynk::connect("0.0.0.0:4222").await?;
    let host_b = HostBuilder::new()
        .with_rpc_client(nc2)
        .with_namespace(NS)
        .with_lattice_cache_provider(REDIS_OCI)
        .build();

    host_b.start().await?;

    actix_rt::time::sleep(Duration::from_secs(2)).await;

    let echo = Actor::from_file("./tests/modules/echo.wasm")?;
    let actor_id = echo.public_key();
    host_a.start_actor(echo).await?;
    await_actor_count(&host_a, 1, Duration::from_millis(50), 3).await?;

    let web_port = 7002_u32;
    let arc = par_from_file("./tests/modules/httpserver.par.gz")?;
    let websrv = NativeCapability::from_archive(&arc, None)?;

    host_b.start_native_capability(websrv).await?;
    // always have to remember that "extras" and kvcache is in the provider list.
    await_provider_count(&host_b, 3, Duration::from_millis(50), 3).await?;

    let nc3 = nats::asynk::connect("0.0.0.0:4222").await?;
    let host_c = HostBuilder::new()
        .with_rpc_client(nc3)
        .with_lattice_cache_provider(REDIS_OCI)
        .with_namespace(NS)
        .build();

    host_c.start().await?;
    actix_rt::time::sleep(Duration::from_secs(2)).await;
    println!("3 hosts started");

    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));
    host_c
        .set_link(
            &actor_id,
            "wasmcloud:httpserver",
            None,
            arc.claims().unwrap().subject.to_string(),
            webvalues,
        )
        .await?;
    println!("set link (activating web server)");
    // let the HTTP server spin up
    actix_rt::time::sleep(Duration::from_secs(1)).await;

    let url = format!("http://localhost:{}/foo/bar", web_port);
    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await?,
               "{\"method\":\"GET\",\"path\":\"/foo/bar\",\"query_string\":\"\",\"headers\":{\"host\":\"localhost:7002\",\"accept\":\"*/*\"},\"body\":[]}");

    host_a.stop().await;
    host_b.stop().await;
    host_c.stop().await;
    actix_rt::time::sleep(Duration::from_millis(500)).await;
    redis::cmd("FLUSHDB").execute(&mut con);
    Ok(())
}

// Run the kvcounter scenario, but with 1 instance of a HTTP provider, 2 instances
// of redis provider,  and 3 instances of the actor in a 5-host lattice.
// We can't do 2 instances of the HTTP provider because it would try and bind the same HTTP port twice
pub(crate) async fn scaled_kvcounter() -> Result<()> {
    // Set the default kvcache provider to enable NATS-based replication
    // by supplying a NATS URL.
    ::std::env::set_var("KVCACHE_NATS_URL", "0.0.0.0:4222");

    use redis::Commands;
    let a = Actor::from_file("./tests/modules/kvcounter.wasm")?;
    let a_id = a.public_key();
    let websrv = par_from_file("./tests/modules/httpserver.par.gz")?;
    let web_id = websrv.claims().as_ref().unwrap().subject.to_string();
    let redis = par_from_file("./tests/modules/redis.par.gz")?;
    let redis_id = redis.claims().as_ref().unwrap().subject.to_string();

    let host_a = scaledkv_host(Some(a), None).await?;
    let host_b = scaledkv_host(
        Some(Actor::from_file("./tests/modules/kvcounter.wasm")?),
        None,
    )
    .await?;
    let host_c = scaledkv_host(
        Some(Actor::from_file("./tests/modules/kvcounter.wasm")?),
        Some(vec![redis]),
    )
    .await?;
    let host_d = scaledkv_host(
        None,
        Some(vec![websrv, par_from_file("./tests/modules/redis.par.gz")?]),
    )
    .await?;
    let host_e = scaledkv_host(
        None,
        Some(vec![par_from_file("./tests/modules/redis.par.gz")?]),
    )
    .await?;
    println!("5 hosts started.");

    let web_port = 6001_u32;

    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));

    let mut redisvalues: HashMap<String, String> = HashMap::new();
    redisvalues.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    host_a
        .set_link(
            &a_id,
            "wasmcloud:httpserver",
            None,
            web_id.to_string(),
            webvalues,
        )
        .await?;

    actix_rt::time::sleep(Duration::from_secs(1)).await;
    host_e
        .set_link(
            &a_id,
            "wasmcloud:keyvalue",
            None,
            redis_id.to_string(),
            redisvalues,
        )
        .await?;

    // let all these hosts stabilize
    actix_rt::time::sleep(Duration::from_secs(3)).await;

    let key = uuid::Uuid::new_v4().to_string();
    let rkey = format!(":{}", key); // the kv wasm logic does a replace on '/' with ':'
    let url = format!("http://localhost:{}/{}", web_port, key);

    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());
    println!("First counter");
    let resp2 = reqwest::get(&url).await?;
    assert_eq!(resp2.text().await?, "{\"counter\":2}");
    println!("Second counter");

    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    let _: () = con.del(&rkey)?;

    host_a.stop().await;
    host_b.stop().await;
    host_c.stop().await;
    host_d.stop().await;
    host_e.stop().await;

    actix_rt::time::sleep(Duration::from_millis(500)).await;

    Ok(())
}

#[cfg(test)]
async fn scaledkv_host(actor: Option<Actor>, par: Option<Vec<ProviderArchive>>) -> Result<Host> {
    const NS: &str = "scaledkvhost";
    let nc = nats::asynk::connect("0.0.0.0:4222").await?;

    let h = HostBuilder::new()
        .with_rpc_client(nc)
        .with_namespace(NS)
        .build();

    h.start().await?;
    if let Some(a) = actor {
        h.start_actor(a).await?;
        await_actor_count(&h, 1, Duration::from_millis(30), 3).await?;
    }
    if let Some(ref vp) = par {
        for p in vp {
            let nc = NativeCapability::from_archive(p, None)?;
            h.start_native_capability(nc).await?;
            actix_rt::time::sleep(Duration::from_millis(50)).await;
        }
        await_provider_count(&h, 2 + vp.len(), Duration::from_millis(30), 3).await?;
    }

    actix_rt::time::sleep(Duration::from_millis(350)).await;

    Ok(h)
}

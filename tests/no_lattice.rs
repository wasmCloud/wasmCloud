use crate::common::{await_actor_count, await_provider_count, gen_kvcounter_host, par_from_file};
use std::collections::HashMap;
use std::time::Duration;
use wasmcloud_actor_http_server::{deserialize, serialize, Request, Response};
use wasmcloud_host::Result;
use wasmcloud_host::{Actor, HostBuilder, NativeCapability};

pub async fn empty_host_has_two_providers() -> Result<()> {
    // Ensure that we're not accidentally using the replication feature on KV cache
    ::std::env::remove_var("KVCACHE_NATS_URL");
    let h = HostBuilder::new().build();
    h.start().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let prov = h.get_providers().await?;
    assert_eq!(2, prov.len());

    Ok(())
}

pub async fn start_and_stop_actor() -> Result<()> {
    ::std::env::remove_var("KVCACHE_NATS_URL");
    let h = HostBuilder::new().build();
    h.start().await?;
    let echo = Actor::from_file("./tests/modules/echo.wasm")?;
    let actor_id = echo.public_key();
    h.start_actor(echo).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 3).await?;

    h.stop_actor(&actor_id).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(0, h.get_actors().await?.len());

    let request = Request {
        method: "GET".to_string(),
        path: "/foo/bar".to_string(),
        query_string: "test=kthxbye".to_string(),
        header: Default::default(),
        body: b"This is a test. Do not be alarmed".to_vec(),
    };
    let buf = serialize(&request)?;
    println!("{}", buf.len());
    let res = h.call_actor(&actor_id, "HandleRequest", &buf).await;
    assert!(res.is_err()); // We should not still be able to call this actor

    Ok(())
}

pub async fn start_and_execute_echo() -> Result<()> {
    // Ensure that we're not accidentally using the replication feature on KV cache
    ::std::env::remove_var("KVCACHE_NATS_URL");
    let h = HostBuilder::new().build();
    h.start().await?;
    let echo = Actor::from_file("./tests/modules/echo.wasm")?;
    let actor_id = echo.public_key();
    h.start_actor(echo).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 3).await?;

    let request = Request {
        method: "GET".to_string(),
        path: "/foo/bar".to_string(),
        query_string: "test=kthxbye".to_string(),
        header: Default::default(),
        body: b"This is a test. Do not be alarmed".to_vec(),
    };
    let buf = serialize(&request)?;
    println!("{}", buf.len());
    let res = h.call_actor(&actor_id, "HandleRequest", &buf).await?;
    println!("{}", res.len());
    let resp: Response = deserialize(&res)?;
    assert_eq!(resp.status_code, 200);
    assert_eq!(resp.status, "OK");
    let v: serde_json::Value = serde_json::from_slice(&resp.body)?;
    assert_eq!("test=kthxbye", v["query_string"].as_str().unwrap());
    h.stop().await;
    Ok(())
}

pub async fn kvcounter_basic() -> Result<()> {
    // Ensure that we're not accidentally using the replication feature on KV cache
    ::std::env::remove_var("KVCACHE_NATS_URL");
    use redis::Commands;

    let h = gen_kvcounter_host(9999, None, None).await?;
    println!("Got host");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let key = uuid::Uuid::new_v4().to_string();
    let rkey = format!(":{}", key); // the kv wasm logic does a replace on '/' with ':'
    let url = format!("http://localhost:9999/{}", key);

    let mut resp = reqwest::get(&url).await?;
    //let mut resp = reqwest::blocking::get(&url)?;
    assert!(resp.status().is_success());
    let _ = reqwest::get(&url).await?;
    resp = reqwest::get(&url).await?; // counter should be at 3 now
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await?, "{\"counter\":3}");
    println!("asserts good");

    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    let _: () = con.del(&rkey)?;
    h.stop().await;

    Ok(())
}

pub async fn actor_to_actor_call_alias() -> Result<()> {
    ::std::env::remove_var("KVCACHE_NATS_URL");
    let h = HostBuilder::new().with_namespace("actor2actor").build();
    h.start().await?;
    // give host time to start
    tokio::time::sleep(Duration::from_secs(3)).await;

    let pinger = Actor::from_file("./tests/modules/pinger.wasm")?;
    let ponger = Actor::from_file("./tests/modules/ponger.wasm")?;
    let pinger_id = pinger.public_key();

    h.start_actor(pinger).await?;
    h.start_actor(ponger).await?;
    await_actor_count(&h, 2, Duration::from_millis(50), 3).await?;

    let arc = par_from_file("./tests/modules/httpserver.par.gz")?;

    let websrv = NativeCapability::from_archive(&arc, None)?;

    let websrv_id = arc.claims().unwrap().subject;

    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", 5091));

    h.start_native_capability(websrv).await?;
    await_provider_count(&h, 3, Duration::from_millis(50), 3).await?;

    h.set_link(
        &pinger_id,
        "wasmcloud:httpserver",
        None,
        websrv_id,
        webvalues,
    )
    .await?;
    // give the web server enough time to fire up
    tokio::time::sleep(Duration::from_millis(150)).await;

    let resp = reqwest::get("http://localhost:5091/foobar").await?;
    assert!(resp.status().is_success());
    assert_eq!("{\"value\":53}", resp.text().await?);

    Ok(())
}

pub async fn kvcounter_start_stop() -> Result<()> {
    // Ensure that we're not accidentally using the replication feature on KV cache
    ::std::env::remove_var("KVCACHE_NATS_URL");
    use redis::Commands;
    let h = gen_kvcounter_host(9997, None, None).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let key = uuid::Uuid::new_v4().to_string();
    let rkey = format!(":{}", key); // the kv wasm logic does a replace on '/' with ':'
    let url = format!("http://localhost:9997/{}", key);

    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());

    h.stop_actor("MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E")
        .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let kvcounter = Actor::from_file("./tests/modules/kvcounter.wasm")?;
    h.start_actor(kvcounter).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 3).await?;

    let arc2 = par_from_file("./tests/modules/httpserver.par.gz")?;

    h.stop_provider(
        &arc2.claims().unwrap().subject,
        "wasmcloud:httpserver",
        None,
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    await_provider_count(&h, 3, Duration::from_millis(50), 4).await?;

    let websrv = NativeCapability::from_archive(&arc2, None)?;
    h.start_native_capability(websrv).await?;
    await_provider_count(&h, 4, Duration::from_millis(50), 3).await?; // 2 providers plus wascc:extras + kvcache
                                                                      // give web server enough time to start
    tokio::time::sleep(Duration::from_millis(300)).await;

    let resp2 = reqwest::get(&url).await?;
    assert!(resp2.status().is_success());
    assert_eq!(resp2.text().await?, "{\"counter\":2}");

    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    let _: () = con.del(&rkey)?;
    h.stop().await;

    Ok(())
}

// Set the link before either the actor or the provider are running in
// the host, and verify that we can then hit the HTTP endpoint.
pub async fn kvcounter_link_first() -> Result<()> {
    // Ensure that we're not accidentally using the replication feature on KV cache
    ::std::env::remove_var("KVCACHE_NATS_URL");
    use redis::Commands;
    let h = HostBuilder::new().build();
    h.start().await?;

    let web_port = 9998_u32;

    // Set the links before there's any provider to invoke OP_BIND_ACTOR

    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));

    let mut values: HashMap<String, String> = HashMap::new();
    values.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    let arc = par_from_file("./tests/modules/redis.par.gz")?;
    let arc2 = par_from_file("./tests/modules/httpserver.par.gz")?;

    let redis_id = arc.claims().unwrap().subject;
    let websrv_id = arc2.claims().unwrap().subject;

    let kvcounter = Actor::from_file("./tests/modules/kvcounter.wasm")?;
    let kvcounter_key = kvcounter.public_key();

    h.set_link(
        &kvcounter_key,
        "wasmcloud:httpserver",
        None,
        websrv_id,
        webvalues,
    )
    .await?;

    h.set_link(&kvcounter_key, "wasmcloud:keyvalue", None, redis_id, values)
        .await?;

    h.start_actor(kvcounter).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 3).await?;

    let redis = NativeCapability::from_archive(&arc, None)?;
    let websrv = NativeCapability::from_archive(&arc2, None)?;

    // When we start these, with pre-existing links, they should trigger OP_BIND_ACTOR invocations
    // for each.
    h.start_native_capability(redis).await?;
    h.start_native_capability(websrv).await?;
    await_provider_count(&h, 4, Duration::from_millis(50), 3).await?; // 2 providers plus wascc:extras
    tokio::time::sleep(Duration::from_millis(150)).await;

    let key = uuid::Uuid::new_v4().to_string();
    let rkey = format!(":{}", key); // the kv wasm logic does a replace on '/' with ':'
    let url = format!("http://localhost:{}/{}", web_port, key);

    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await?, "{\"counter\":1}");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    let _: () = con.del(&rkey)?;
    h.stop().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    Ok(())
}

/// Ensures the embedded extras provider in a wasmcloud host
/// handles operations properly
pub async fn extras_provider() -> Result<()> {
    const WEB_PORT: u32 = 9997_u32;
    const EXTRAS_PUBLIC_KEY: &str = "VDHPKGFKDI34Y4RN4PWWZHRYZ6373HYRSNNEM4UTDLLOGO5B37TSVREP";

    let h = HostBuilder::new().build();
    h.start().await?;
    // Start extras actor
    //TODO(brooksmtownsend): replace this with the real extras
    let extras = Actor::from_file("./tests/modules/extras.wasm")?;
    let extras_id = extras.public_key();
    h.start_actor(extras).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 3).await?;

    // Start httpserver provider
    let mut config_values = HashMap::new();
    config_values.insert("PORT".to_string(), format!("{}", WEB_PORT));
    let httpserver = par_from_file("./tests/modules/httpserver.par.gz")?;
    let websrv = NativeCapability::from_archive(&httpserver, None)?;
    h.start_native_capability(websrv).await?;
    await_provider_count(&h, 3, Duration::from_millis(50), 3).await?; // httpserver, kvcache, extras
    h.set_link(
        &extras_id,
        "wasmcloud:httpserver",
        None,
        httpserver.claims().unwrap().subject,
        config_values,
    )
    .await?;
    h.set_link(
        &extras_id,
        "wasmcloud:extras",
        None,
        EXTRAS_PUBLIC_KEY.to_string(),
        HashMap::new(),
    )
    .await?;
    // give web server enough time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Query extras actor
    #[derive(Debug, serde::Deserialize)]
    struct ExtrasActorResponse {
        guid: String,
        random: u32,
        sequence: u64,
    }
    let url = format!("http://localhost:{}/extras", WEB_PORT);
    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());
    let generator_result: ExtrasActorResponse = resp.json().await?;
    assert!(uuid::Uuid::parse_str(&generator_result.guid).is_ok());
    assert!(generator_result.random < 100);
    assert_eq!(generator_result.sequence, 0);

    Ok(())
}

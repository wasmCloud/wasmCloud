use crate::common::{await_actor_count, await_provider_count, gen_kvcounter_host, par_from_file};
use crate::generated::http::{deserialize, serialize, Request, Response};
use provider_archive::ProviderArchive;
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Write};
use std::time::Duration;
use wasmcloud_host::Result;
use wasmcloud_host::{Actor, HostBuilder, NativeCapability};

pub async fn start_and_execute_echo() -> Result<()> {
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
    use redis::Commands;

    let h = gen_kvcounter_host(9999, None, None).await?;
    println!("Got host");

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

    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    let _: () = con.del(&rkey)?;
    h.stop().await;

    Ok(())
}

pub async fn kvcounter_start_stop() -> Result<()> {
    use redis::Commands;
    let h = gen_kvcounter_host(9997, None, None).await?;

    let key = uuid::Uuid::new_v4().to_string();
    let rkey = format!(":{}", key); // the kv wasm logic does a replace on '/' with ':'
    let url = format!("http://localhost:9997/{}", key);

    let resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());

    h.stop_actor("MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ")
        .await?;
    println!("Waiting for 0");
    await_actor_count(&h, 0, Duration::from_millis(50), 3).await?;
    println!("Got 0");

    let kvcounter = Actor::from_file("./tests/modules/kvcounter.wasm")?;
    h.start_actor(kvcounter).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 3).await?;

    let arc2 = par_from_file("./tests/modules/libwascc_httpsrv.par.gz")?;

    h.stop_provider(&arc2.claims().unwrap().subject, "wascc:http_server", None)
        .await?;
    await_provider_count(&h, 2, Duration::from_millis(50), 3).await?;

    ::std::thread::sleep(Duration::from_millis(50)); // give the web server enough time to let go of the port

    let websrv = NativeCapability::from_archive(&arc2, None)?;
    h.start_native_capability(websrv).await?;
    await_provider_count(&h, 3, Duration::from_millis(50), 3).await?; // 2 providers plus wascc:extras

    let resp2 = reqwest::get(&url).await?;
    assert!(resp2.status().is_success());
    assert_eq!(resp2.text().await?, "{\"counter\":2}");

    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    let _: () = con.del(&rkey)?;
    h.stop().await;

    Ok(())
}

// Set the binding before either the actor or the provider are running in
// the host, and verify that we can then hit the HTTP endpoint.
pub async fn kvcounter_binding_first() -> Result<()> {
    use redis::Commands;
    let h = HostBuilder::new().build();
    h.start().await?;

    let web_port = 9998_u32;

    // Set the bindings before there's any provider to invoke OP_BIND_ACTOR

    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));

    let mut values: HashMap<String, String> = HashMap::new();
    values.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    let arc = par_from_file("./tests/modules/libwascc_redis.par.gz")?;
    let arc2 = par_from_file("./tests/modules/libwascc_httpsrv.par.gz")?;

    let redis_id = arc.claims().unwrap().subject;
    let websrv_id = arc2.claims().unwrap().subject;

    let kvcounter = Actor::from_file("./tests/modules/kvcounter.wasm")?;
    let kvcounter_key = kvcounter.public_key();

    h.set_binding(
        &kvcounter_key,
        "wascc:http_server",
        None,
        websrv_id,
        webvalues,
    )
    .await?;

    h.set_binding(&kvcounter_key, "wascc:keyvalue", None, redis_id, values)
        .await?;

    h.start_actor(kvcounter).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 3).await?;

    let redis = NativeCapability::from_archive(&arc, None)?;
    let websrv = NativeCapability::from_archive(&arc2, None)?;

    // When we start these, with pre-existing bindings, they should trigger OP_BIND_ACTOR invocations
    // for each.
    h.start_native_capability(redis).await?;
    h.start_native_capability(websrv).await?;
    await_provider_count(&h, 3, Duration::from_millis(50), 3).await?; // 2 providers plus wascc:extras

    let key = uuid::Uuid::new_v4().to_string();
    let rkey = format!(":{}", key); // the kv wasm logic does a replace on '/' with ':'
    let url = format!("http://localhost:{}/{}", web_port, key);

    let mut resp = reqwest::get(&url).await?;
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await?, "{\"counter\":1}");

    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    let _: () = con.del(&rkey)?;
    h.stop().await;

    Ok(())
}

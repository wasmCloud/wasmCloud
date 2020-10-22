use crate::common::{await_actor_count, await_provider_count, par_from_file};
use crate::generated::http::{deserialize, serialize, Request, Response};
use provider_archive::ProviderArchive;
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Write};
use std::time::Duration;
use wascc_host::{Actor, HostBuilder, NativeCapability};

pub async fn start_and_execute_echo() -> Result<(), Box<dyn Error + Sync + Send>> {
    let h = HostBuilder::new().build();
    h.start(None, None).await?;
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
    Ok(())
}

pub async fn kvcounter_basic() -> Result<(), Box<dyn Error + Sync + Send>> {
    use redis::Commands;

    let h = HostBuilder::new().build();
    h.start(None, None).await?;

    let kvcounter = Actor::from_file("./tests/modules/kvcounter.wasm")?;
    let kvcounter_key = kvcounter.public_key();
    h.start_actor(kvcounter).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 3).await?;

    let arc = par_from_file("./tests/modules/libwascc_redis.par")?;
    let arc2 = par_from_file("./tests/modules/libwascc_httpsrv.par")?;

    let redis = NativeCapability::from_archive(&arc, None)?;
    let websrv = NativeCapability::from_archive(&arc2, None)?;

    let redis_id = arc.claims().unwrap().subject;
    let websrv_id = arc2.claims().unwrap().subject;

    let mut values: HashMap<String, String> = HashMap::new();
    values.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), "9999".to_string());
    h.start_native_capability(redis).await?;
    h.start_native_capability(websrv).await?;
    // need to wait for 3 providers because extras is always there
    await_provider_count(&h, 3, Duration::from_millis(500), 5).await?;

    h.set_binding(&kvcounter_key, "wascc:keyvalue", None, redis_id, values)
        .await?;

    h.set_binding(
        &kvcounter_key,
        "wascc:http_server",
        None,
        websrv_id,
        webvalues,
    )
    .await?;

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

    Ok(())
}

use provider_archive::ProviderArchive;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::time::Duration;
use wasmcloud_host::{Actor, Host, HostBuilder, NativeCapability, Result};

pub const REDIS_OCI: &str = "wascc.azurecr.io/redis:v0.9.2";
pub const HTTPSRV_OCI: &str = "wascc.azurecr.io/httpsrv:v0.9.2";
pub const NATS_OCI: &str = "wascc.azurecr.io/nats:v0.9.1";
pub const KVCOUNTER_OCI: &str = "wascc.azurecr.io/kvcounter:v0.1.1";

pub async fn await_actor_count(
    h: &Host,
    count: usize,
    backoff: Duration,
    max_attempts: i32,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        match actix_rt::time::timeout(backoff, h.get_actors()).await {
            Ok(c) => {
                if c.unwrap().len() >= count {
                    break;
                }
            }
            Err(_e) => {
                if attempt > max_attempts {
                    return Err("Exceeded max attempts".into());
                }
            }
        }
        attempt = attempt + 1;
    }
    Ok(())
}

pub async fn await_provider_count(
    h: &Host,
    count: usize,
    backoff: Duration,
    max_attempts: i32,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        match actix_rt::time::timeout(backoff, h.get_providers()).await {
            Ok(c) => {
                if c.unwrap().len() >= count {
                    break;
                }
            }
            Err(_e) => {
                if attempt > max_attempts {
                    return Err("Exceeded max attempts".into());
                }
            }
        }
        attempt = attempt + 1;
    }
    Ok(())
}

pub async fn gen_kvcounter_host(
    web_port: u32,
    lattice_rpc: Option<nats::asynk::Connection>,
    lattice_control: Option<nats::asynk::Connection>,
) -> Result<Host> {
    let mut h = HostBuilder::new();
    if let Some(rpc) = lattice_rpc {
        h = h.with_rpc_client(rpc);
    }
    if let Some(cplane) = lattice_control {
        h = h.with_control_client(cplane);
    }
    let h = h.build();
    h.start().await?;

    let kvcounter = Actor::from_file("./tests/modules/kvcounter.wasm")?;
    let kvcounter_key = kvcounter.public_key();
    h.start_actor(kvcounter).await?;
    await_actor_count(&h, 1, Duration::from_millis(50), 3).await?;

    let arc = par_from_file("./tests/modules/libwascc_redis.par.gz")?;
    let arc2 = par_from_file("./tests/modules/libwascc_httpsrv.par.gz")?;

    let redis = NativeCapability::from_archive(&arc, None)?;
    let websrv = NativeCapability::from_archive(&arc2, None)?;

    let redis_id = arc.claims().unwrap().subject;
    let websrv_id = arc2.claims().unwrap().subject;

    let mut values: HashMap<String, String> = HashMap::new();
    values.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));
    h.start_native_capability(redis).await?;
    h.start_native_capability(websrv).await?;
    await_provider_count(&h, 3, Duration::from_millis(50), 3).await?; // 2 providers plus wascc:extras
    h.set_link(&kvcounter_key, "wascc:keyvalue", None, redis_id, values)
        .await?;

    h.set_link(
        &kvcounter_key,
        "wascc:http_server",
        None,
        websrv_id,
        webvalues,
    )
    .await?;

    Ok(h)
}

pub fn par_from_file(file: &str) -> Result<ProviderArchive> {
    let mut f = File::open(file)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    ProviderArchive::try_load(&buf)
}

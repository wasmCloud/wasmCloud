use actix_rt::time::delay_for;
use crossbeam_channel::{Receiver, Sender};
use provider_archive::ProviderArchive;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::time::Duration;
use wasmcloud_host::{Actor, ControlEvent, Host, HostBuilder, NativeCapability, Result};

pub const REDIS_OCI: &str = "wasmcloud.azurecr.io/redis:0.11.1";
pub const REDIS_KEY: &str = "VAZVC4RX54J2NVCMCW7BPCAHGGG5XZXDBXFUMDUXGESTMQEJLC3YVZWB";

pub const HTTPSRV_OCI: &str = "wasmcloud.azurecr.io/httpserver:0.11.1";
pub const HTTPSRV_KEY: &str = "VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M";

pub const NATS_OCI: &str = "wasmcloud.azurecr.io/nats:0.10.1";
pub const KVCOUNTER_OCI: &str = "wasmcloud.azurecr.io/kvcounter:0.2.0";

pub const KVCOUNTER_KEY: &str = "MCFMFDWFHGKELOXPCNCDXKK5OFLHBVEWRAOXR5JSQUD2TOFRE3DFPM7E";

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
                    println!("PROVIDER COUNT FAIL AT {}/{}", attempt, max_attempts);
                    return Err("Exceeded max attempts".into());
                }
            }
        }
        attempt = attempt + 1;
    }
    Ok(())
}

pub async fn gen_kvcounter_host(h: &Host, web_port: u32, collector: &EventCollector) -> Result<()> {
    let timeout = Duration::from_secs(4); // this can take long because of OCI downloads
                                          /*    if let Some(rpc) = lattice_rpc {
                                              h = h.with_rpc_client(rpc);
                                          }
                                          if let Some(cplane) = lattice_control {
                                              h = h.with_control_client(cplane);
                                          } */

    h.start().await?;
    assert!(collector.wait_for(|e| *e == ControlEvent::HostStarted, timeout)?);

    //let kvcounter = Actor::from_file("./tests/modules/kvcounter.wasm")?;
    h.start_actor_from_registry(KVCOUNTER_OCI).await?;

    assert!(collector.wait_for(
        |e| *e
            == ControlEvent::ActorStarted {
                actor: KVCOUNTER_KEY.to_string(),
                image_ref: Some(KVCOUNTER_OCI.to_string())
            },
        timeout
    )?);

    let mut values: HashMap<String, String> = HashMap::new();
    values.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    let mut webvalues: HashMap<String, String> = HashMap::new();
    webvalues.insert("PORT".to_string(), format!("{}", web_port));

    //let arc = par_from_file("./tests/modules/redis.par.gz")?;
    h.start_capability_from_registry(REDIS_OCI, None).await?;
    //let arc2 = par_from_file("./tests/modules/httpserver.par.gz")?;
    h.start_capability_from_registry(HTTPSRV_OCI, None).await?;

    //let redis = NativeCapability::from_archive(&arc, None)?;
    //let websrv = NativeCapability::from_archive(&arc2, None)?;

    assert!(collector.wait_for(
        |e| *e
            == ControlEvent::ProviderStarted {
                contract_id: "wasmcloud:keyvalue".to_string(),
                image_ref: Some(REDIS_OCI.to_string()),
                link_name: "default".to_string(),
                provider_id: REDIS_KEY.to_string()
            },
        timeout
    )?);

    assert!(collector.wait_for(
        |e| *e
            == ControlEvent::ProviderStarted {
                contract_id: "wasmcloud:httpserver".to_string(),
                image_ref: Some(HTTPSRV_OCI.to_string()),
                link_name: "default".to_string(),
                provider_id: HTTPSRV_KEY.to_string()
            },
        timeout
    )?);

    println!("HERE");
    //await_provider_count(&h, 4, Duration::from_millis(50), 3).await?; // 2 providers plus wasmcloud:extras
    h.set_link(
        KVCOUNTER_KEY,
        "wasmcloud:keyvalue",
        None,
        REDIS_KEY.to_string(),
        values,
    )
    .await?;
    assert!(collector.wait_for(
        |e| *e
            == ControlEvent::LinkEstablished {
                contract_id: "wasmcloud:keyvalue".to_string(),
                link_name: "default".to_string(),
                provider_id: REDIS_KEY.to_string()
            },
        timeout
    )?);

    println!("HERE2");
    h.set_link(
        KVCOUNTER_KEY,
        "wasmcloud:httpserver",
        None,
        HTTPSRV_KEY.to_string(),
        webvalues,
    )
    .await?;

    assert!(collector.wait_for(
        |e| *e
            == ControlEvent::LinkEstablished {
                contract_id: "wasmcloud:httpserver".to_string(),
                link_name: "default".to_string(),
                provider_id: HTTPSRV_KEY.to_string()
            },
        timeout
    )?);

    Ok(())
}

pub fn par_from_file(file: &str) -> Result<ProviderArchive> {
    let mut f = File::open(file)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    ProviderArchive::try_load(&buf)
}

pub struct EventCollector {
    receiver: Receiver<ControlEvent>,
}

impl EventCollector {
    pub fn new(receiver: Receiver<ControlEvent>) -> Self {
        EventCollector { receiver }
    }

    pub fn wait_for(
        &self,
        pred: impl Fn(&ControlEvent) -> bool,
        timeout: Duration,
    ) -> Result<bool> {
        match self.receiver.recv_timeout(timeout) {
            Ok(ref e) => match pred(e) {
                true => Ok(true),
                false => {
                    println!("Event collector predicate failed, received {:?}", e);
                    Ok(false)
                }
            },
            Err(e) => Err(format!("Event collector expectation failed: {}", e).into()),
        }
    }
}

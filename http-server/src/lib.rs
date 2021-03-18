use actix_rt;
use actix_web::dev::Body;
use actix_web::http::{HeaderName, HeaderValue, StatusCode};
use actix_web::web::Bytes;
use actix_web::{middleware, web, App, HttpRequest, HttpResponse, HttpServer};
use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::{OP_BIND_ACTOR, OP_HEALTH_REQUEST, OP_REMOVE_ACTOR};
#[allow(unused_imports)]
use log::{debug, error, info, trace};
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;
use wasmcloud_actor_core::{deserialize, serialize, CapabilityConfiguration, HealthCheckResponse};
use wasmcloud_actor_http_server::{Request, Response};
use wasmcloud_provider_core as codec;
#[cfg(not(feature = "static_plugin"))]
use wasmcloud_provider_core::capability_provider;

#[allow(unused)]
const CAPABILITY_ID: &str = "wasmcloud:httpserver";

const OP_HANDLE_REQUEST: &str = "HandleRequest";

// The module id (agent's public key) has to be passed around between threads
// and must implement the Copy trait, which String doesn't, but fixed-length vectors do,
// so we'll use ModuleId defined here to store it.
type ModuleId = [u8; MODULE_ID_LEN];
const MODULE_ID_LEN: usize = 56;

#[cfg(not(feature = "static_plugin"))]
capability_provider!(HttpServerProvider, HttpServerProvider::new);

/// An Actix-web implementation of the `wasmcloud:httpserver` capability specification
#[derive(Clone)]
pub struct HttpServerProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    servers: Arc<RwLock<HashMap<ModuleId, oneshot::Sender<()>>>>,
}

impl HttpServerProvider {
    /// Creates a new HTTP server provider. This is automatically invoked
    /// by dynamically loaded plugins, and manually invoked by custom hosts
    /// with a statically-linked dependency on this crate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops a running web server, freeing up its associated port
    fn terminate_server(&self, module_id: &ModuleId) {
        let module = String::from_utf8_lossy(module_id);
        {
            let lock = self.servers.read().unwrap();
            if !lock.contains_key(module_id) {
                error!(
                    "Received request to stop server for non-configured actor {}. Igoring.",
                    module
                );
                return;
            }
        }
        info!("Stopping httpserver {}", module);
        {
            let mut lock = self.servers.write().unwrap();
            if let Some(tx) = lock.remove(module_id) {
                if let Err(_) = tx.send(()) {
                    error!("Kill command for HttpServer was dropped");
                }
            }
        }
    }

    /// Starts a new web server and binds to the appropriate port
    fn spawn_server(&self, cfgvals: &CapabilityConfiguration) {
        let module_id = match ModuleId::try_from(&cfgvals.module) {
            Ok(id) => id,
            Err(_) => {
                error!("Invalid module id {}", &cfgvals.module);
                return;
            }
        };
        if self.servers.read().unwrap().contains_key(&module_id) {
            return;
        }
        info!("Received HTTP Server configuration for {}", &cfgvals.module);

        // The optional BIND parameter is a comma-separated list of values of the form:
        //   either "IP:PORT" or "PORT", where IP is an IPV4 or IPV6 address, and PORT is a u16
        // If no BIND (or PORT) is specified, a default of '0.0.0.0:8080' is used.
        // All binds must succeed for the server to start
        let mut bind_addresses = Vec::new();
        if let Some(vals) = cfgvals.values.get("BIND") {
            for addr in vals.split(',') {
                if addr.parse::<u16>().is_ok() {
                    bind_addresses.push(format!("0.0.0.0:{}", addr))
                } else {
                    bind_addresses.push(addr.to_string())
                }
            }
        }
        // The optional PORT parameter is a single port. If specified, the address '0.0.0.0' is used.
        if let Some(port) = cfgvals.values.get("PORT") {
            bind_addresses.push(format!("0.0.0.0:{}", port))
        }
        if bind_addresses.is_empty() {
            bind_addresses.push("0.0.0.0:8080".to_string())
        }
        // The optional WORKERS parameter specifies the number of worker threads to spawn.
        // If not specified, actix_web uses the number of logical cpus.
        // If the parameter is not a valid integer, the default will be used.
        let workers = match cfgvals.values.get("WORKERS") {
            Some(v) => match v.parse::<usize>() {
                Ok(v) => Some(v),
                Err(e) => {
                    error!("Invalid value for WORKERS '{}' (err={}), ignoring parameter and will use # logical cpus", v, e);
                    None
                }
            },
            None => None,
        };
        let (stop_tx, stop_rx) = oneshot::channel();
        let disp = self.dispatcher.clone();
        let module = module_id.clone();
        std::thread::spawn(move || {
            let sys = actix_rt::System::new();
            let mut server = HttpServer::new(move || {
                App::new()
                    .wrap(middleware::Logger::default())
                    .data(disp.clone())
                    .data(module.clone())
                    .default_service(web::route().to(request_handler))
            })
            .disable_signals();
            for addr in bind_addresses.iter() {
                server = match server.bind(addr) {
                    Ok(server) => {
                        debug!("HttpServer configured for {}", addr);
                        server
                    }
                    Err(e) => {
                        error!("Invalid HttpServer bind address: {}: {}", addr, e);
                        return;
                    }
                }
            }
            if let Some(num) = workers {
                debug!("HttpServer configured for {} workers", num);
                server = server.workers(num);
            }
            sys.block_on(async move {
                // start the worker threads
                let server = server.run();

                // wait for kill signal
                if let Err(e) = stop_rx.await {
                    error!("Unexpected error in HttpServer thread .. {}", e);
                } else {
                    info!(
                        "Stop signal received, stopping HttpServer for {}",
                        &String::from_utf8_lossy(&module)
                    );
                    server.stop(true).await;
                }
            });
            trace!("HttpServer startup thread exiting");
        });
        self.servers.write().unwrap().insert(module_id, stop_tx);
    }
}

impl Default for HttpServerProvider {
    fn default() -> Self {
        if env_logger::try_init().is_err() {}
        HttpServerProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            servers: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

trait TryFrom<T> {
    type Error;
    fn try_from(val: T) -> Result<Self, Self::Error>
    where
        Self: std::marker::Sized;
}

impl TryFrom<&str> for ModuleId {
    type Error = String;
    fn try_from(val: &str) -> Result<Self, Self::Error> {
        let mut id: ModuleId = [0u8; MODULE_ID_LEN];
        if val.as_bytes().len() == id.len() {
            id.copy_from_slice(val.as_bytes());
            Ok(id)
        } else {
            Err("Module id must be exactly 56 chars".to_string())
        }
    }
}

impl CapabilityProvider for HttpServerProvider {
    /// Accepts the dispatcher provided by the waSCC host runtime
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn Error + Sync + Send>> {
        info!("Dispatcher configured.");

        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    /// Handles an invocation from the host runtime
    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        trace!("Handling operation `{}` from `{}`", op, actor);

        match op {
            OP_BIND_ACTOR if actor == "system" => {
                self.spawn_server(&deserialize(msg)?);
                Ok(vec![])
            }
            OP_REMOVE_ACTOR if actor == "system" => {
                let cfgvals = deserialize::<CapabilityConfiguration>(msg)?;
                info!("Removing actor configuration for {}", cfgvals.module);
                let module_id = ModuleId::try_from(&cfgvals.module)?;
                self.terminate_server(&module_id);
                Ok(vec![])
            }
            OP_HEALTH_REQUEST if actor == "system" => Ok(serialize(HealthCheckResponse {
                healthy: true,
                message: "".to_string(),
            })?),
            _ => Err("bad dispatch".into()),
        }
    }

    fn stop(&self) {
        let server_list: Vec<_> = {
            let lock = self.servers.read().unwrap();
            lock.keys().cloned().collect()
        };
        for svr in server_list {
            self.terminate_server(&svr);
        }
    }
}

async fn request_handler(
    req: HttpRequest,
    payload: Bytes,
    disp: web::Data<Arc<RwLock<Box<dyn Dispatcher>>>>,
    module: web::Data<ModuleId>,
) -> HttpResponse {
    let request = Request {
        method: req.method().as_str().to_string(),
        path: req.uri().path().to_string(),
        query_string: req.query_string().to_string(),
        header: extract_headers(&req),
        body: payload.to_vec(),
    };
    let buf = serialize(request).unwrap();

    let resp = {
        let lock = (*disp).read().unwrap();
        lock.dispatch(
            &String::from_utf8_lossy(module.get_ref()),
            OP_HANDLE_REQUEST,
            &buf,
        )
    };
    match resp {
        Ok(r) => {
            let r = deserialize::<Response>(r.as_slice());
            if let Ok(r) = r {
                let mut response = HttpResponse::with_body(
                    StatusCode::from_u16(r.status_code as _).unwrap(),
                    Body::from_slice(&r.body),
                );
                if !r.header.is_empty() {
                    let headers = response.head_mut();
                    r.header.iter().for_each(|(key, val)| {
                        let _ = headers.headers.insert(
                            HeaderName::from_bytes(key.as_bytes()).unwrap(),
                            HeaderValue::from_str(val).unwrap(),
                        );
                    });
                }
                response
            } else {
                HttpResponse::InternalServerError().body("Malformed response from actor")
            }
        }
        Err(e) => {
            error!("Guest failed to handle HTTP request: {}", e);
            HttpResponse::with_body(
                StatusCode::from_u16(500u16).unwrap(),
                Body::from_slice(b"Failed to handle request"),
            )
        }
    }
}

fn extract_headers(req: &HttpRequest) -> HashMap<String, String> {
    let mut hm = HashMap::new();

    for (hname, hval) in req.headers().iter() {
        hm.insert(
            hname.as_str().to_string(),
            hval.to_str().unwrap().to_string(),
        );
    }

    hm
}

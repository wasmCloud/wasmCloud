#[macro_use]
extern crate wascc_codec as codec;
#[macro_use]
extern crate log;

extern crate actix_rt;
use actix_web::dev::Body;
use actix_web::dev::Server;
use actix_web::http::{HeaderName, HeaderValue, StatusCode};
use actix_web::web::Bytes;
use actix_web::{middleware, web, App, HttpRequest, HttpResponse, HttpServer};
use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::{OP_BIND_ACTOR, OP_HEALTH_REQUEST, OP_REMOVE_ACTOR};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::RwLock;
use wasmcloud_actor_core::{deserialize, serialize, CapabilityConfiguration, HealthCheckResponse};
use wasmcloud_actor_http_server::{Request, Response};

#[allow(unused)]
const CAPABILITY_ID: &str = "wasmcloud:httpserver";

const OP_HANDLE_REQUEST: &str = "HandleRequest";

#[cfg(not(feature = "static_plugin"))]
capability_provider!(HttpServerProvider, HttpServerProvider::new);

/// An Actix-web implementation of the `wasmcloud:httpserver` capability specification
#[derive(Clone)]
pub struct HttpServerProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    servers: Arc<RwLock<HashMap<String, Server>>>,
}

impl HttpServerProvider {
    /// Creates a new HTTP server provider. This is automatically invoked
    /// by dynamically loaded plugins, and manually invoked by custom hosts
    /// with a statically-linked dependency on this crate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops a running web server, freeing up its associated port
    fn terminate_server(&self, module: &str) {
        {
            let lock = self.servers.read().unwrap();
            if !lock.contains_key(module) {
                error!(
                    "Received request to stop server for non-configured actor {}. Igoring.",
                    module
                );
                return;
            }
            let server = lock.get(module).unwrap();
            let _ = server.stop(true);
        }
        {
            let mut lock = self.servers.write().unwrap();
            lock.remove(module).unwrap();
        }
    }

    /// Starts a new web server and binds to the appropriate port
    fn spawn_server(&self, cfgvals: &CapabilityConfiguration) {
        if self.servers.read().unwrap().contains_key(&cfgvals.module) {
            return;
        }
        let bind_addr = match cfgvals.values.get("PORT") {
            Some(v) => format!("0.0.0.0:{}", v),
            None => "0.0.0.0:8080".to_string(),
        };

        let disp = self.dispatcher.clone();
        let module_id = cfgvals.module.clone();

        info!("Received HTTP Server configuration for {}", module_id);
        let servers = self.servers.clone();

        std::thread::spawn(move || {
            let module = module_id.clone();
            let sys = actix_rt::System::new(&module);
            let server = HttpServer::new(move || {
                App::new()
                    .wrap(middleware::Logger::default())
                    .data(disp.clone())
                    .data(module.clone())
                    .default_service(web::route().to(request_handler))
            })
            .bind(bind_addr)
            .unwrap()
            .disable_signals()
            .run();

            servers.write().unwrap().insert(module_id.clone(), server);

            let _ = sys.run();
        });
    }
}

impl Default for HttpServerProvider {
    fn default() -> Self {
        match env_logger::try_init() {
            Ok(_) => {}
            Err(_) => {}
        };
        HttpServerProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            servers: Arc::new(RwLock::new(HashMap::new())),
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
                self.terminate_server(&cfgvals.module);
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
    state: web::Data<Arc<RwLock<Box<dyn Dispatcher>>>>,
    module: web::Data<String>,
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
        let lock = (*state).read().unwrap();
        lock.dispatch(module.get_ref(), OP_HANDLE_REQUEST, &buf)
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
                        headers.headers.insert(
                            HeaderName::from_bytes(key.as_bytes()).unwrap(),
                            HeaderValue::from_str(val).unwrap(),
                        )
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

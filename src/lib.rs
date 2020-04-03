// Copyright 2015-2019 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[macro_use]
extern crate wascc_codec as codec;

#[macro_use]
extern crate log;

extern crate actix_rt;

use actix_web::dev::Body;
use actix_web::dev::Server;
use actix_web::http::StatusCode;
use actix_web::web::Bytes;
use actix_web::{middleware, web, App, HttpRequest, HttpResponse, HttpServer};
use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::CapabilityConfiguration;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;
use std::sync::RwLock;
use wascc_codec::core::OP_CONFIGURE;
use wascc_codec::core::OP_REMOVE_ACTOR;
use wascc_codec::{deserialize, serialize};

/// Unique identifier for the capability being provided. Note other providers can
/// provide this same capability (just not at the same time)
const CAPABILITY_ID: &str = "wascc:http_server";

capability_provider!(HttpServerProvider, HttpServerProvider::new);

pub struct HttpServerProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    servers: Arc<RwLock<HashMap<String, Server>>>,
}

impl HttpServerProvider {
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
            let _ = server
                .stop(true);
        }
        {
            let mut lock = self.servers.write().unwrap();
            lock.remove(module).unwrap();
        }
    }

    /// Starts a new web server and binds to the appropriate port
    fn spawn_server(&self, cfgvals: &CapabilityConfiguration) {
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
            Err(_) => println!("** HTTP provider: Logger already initialized, skipping."),
        };
        HttpServerProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            servers: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl CapabilityProvider for HttpServerProvider {
    fn capability_id(&self) -> &'static str {
        CAPABILITY_ID
    }

    fn configure_dispatch(&self, dispatcher: Box<dyn Dispatcher>) -> Result<(), Box<dyn StdError>> {
        info!("Dispatcher configured.");

        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "waSCC Default HTTP Server (Actix Web)"
    }

    fn handle_call(
        &self,
        origin: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn StdError>> {
        info!("Handling operation `{}` from `{}`", op, origin);
        // TIP: do not allow individual modules to attempt to send configuration,
        // only accept it from the host runtime
        if op == OP_CONFIGURE && origin == "system" {
            let cfgvals = deserialize(msg)?;
            self.spawn_server(&cfgvals);
            Ok(vec![])
        } else if op == OP_REMOVE_ACTOR && origin == "system" {
            let cfgvals = deserialize::<CapabilityConfiguration>(msg)?;
            info!("Removing actor configuration for {}", cfgvals.module);
            self.terminate_server(&cfgvals.module);
            Ok(vec![])
        } else {
            Err(format!("Unknown operation: {}", op).into())
        }
    }
}

async fn request_handler(
    req: HttpRequest,
    payload: Bytes,
    state: web::Data<Arc<RwLock<Box<dyn Dispatcher>>>>,
    module: web::Data<String>,
) -> HttpResponse {
    let request = codec::http::Request {
        method: req.method().as_str().to_string(),
        path: req.uri().path().to_string(),
        query_string: req.query_string().to_string(),
        header: extract_headers(&req),
        body: payload.to_vec(),
    };
    let buf = serialize(request).unwrap();

    let resp = {
        let lock = (*state).read().unwrap();
        lock.dispatch(&format!("{}!HandleRequest", module.get_ref()), &buf)
    };
    match resp {
        Ok(r) => {
            let r = deserialize::<codec::http::Response>(r.as_slice()).unwrap();
            HttpResponse::with_body(
                StatusCode::from_u16(r.status_code as _).unwrap(),
                Body::from_slice(&r.body),
            )
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

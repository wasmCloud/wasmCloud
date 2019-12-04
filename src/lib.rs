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

use actix_web::dev::Body;
use actix_web::http::StatusCode;
use actix_web::{middleware, web, App, HttpRequest, HttpResponse, HttpServer};
use bytes::Bytes;
use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::CapabilityConfiguration;
use prost::Message;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;
use std::sync::RwLock;
use wascc_codec::core::OP_CONFIGURE;

/// Unique identifier for the capability being provided. Note other providers can
/// provide this same capability (just not at the same time)
const CAPABILITY_ID: &str = "wascc:http_server";

capability_provider!(HttpServerProvider, HttpServerProvider::new);

pub struct HttpServerProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
}

impl HttpServerProvider {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for HttpServerProvider {
    fn default() -> Self {
        env_logger::init();
        HttpServerProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
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
        "Wascc Default HTTP Server"
    }

    fn handle_call(&self, op: &str, msg: &[u8]) -> Result<Vec<u8>, Box<dyn StdError>> {
        info!("Handling operation: {}", op);
        if op == OP_CONFIGURE {
            let cfgvals = CapabilityConfiguration::decode(msg)?;
            let bind_addr = match cfgvals.values.get("PORT") {
                Some(v) => format!("0.0.0.0:{}", v),
                None => "0.0.0.0:8080".to_string(),
            };

            let disp = self.dispatcher.clone();

            info!("Received HTTP Server configuration for {}", cfgvals.module);

            std::thread::spawn(move || {
                HttpServer::new(move || {
                    App::new()
                        .wrap(middleware::Logger::default())
                        .data(disp.clone())
                        .data(cfgvals.module.clone())
                        .default_service(web::route().to(request_handler))
                })
                .bind(bind_addr)
                .unwrap()
                .disable_signals()
                .run()
                .unwrap();
            });
        }
        Ok(vec![])
    }
}

fn request_handler(
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
    let mut buf = Vec::new();
    request.encode(&mut buf).unwrap();

    let resp = {
        let lock = (*state).read().unwrap();
        lock.dispatch(&format!("{}!HandleRequest", module.get_ref()), &buf)
    };
    match resp {
        Ok(r) => {
            let r = codec::http::Response::decode(&r).unwrap();
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

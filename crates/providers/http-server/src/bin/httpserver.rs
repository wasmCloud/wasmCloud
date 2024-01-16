//! HTTP Server implementation for wasmcloud:httpserver
//!
//!

use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk;

use wasmcloud_provider_httpserver::HttpServerProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // start_provider initializes the threaded tokio executor and sets up
    // the provider to listen to RPC and participate in a wasmcloud lattice
    wasmcloud_provider_sdk::start_provider(
        HttpServerProvider::default(),
        Some("http-server-provider".to_string()),
    )?;

    eprintln!("HttpServer provider exiting");
    Ok(())
}

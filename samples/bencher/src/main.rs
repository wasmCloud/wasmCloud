use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::time::Instant;
use wasmcloud_actor_core::serialize;
use wasmcloud_actor_http_server as http;
use wasmcloud_host::{Actor, HostBuilder};

#[actix_rt::main]
async fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    let h = HostBuilder::new().build();
    h.start().await?;
    let echo = match Actor::from_file(
        "../../target/wasm32-unknown-unknown/release/bench_actor_s.wasm",
    ) {
        Ok(e) => e,
        Err(e) => {
            println!("Unable to locate bench_actor_s.wasm. Please run 'make release' in samples/bench-actor/");
            return Err(e);
        }
    };
    let actor_id = echo.public_key();
    h.start_actor(echo).await?;

    let data = Data {
        field1: 21.5,
        field2: 30,
        field3: "testing".to_string(),
    };
    let payload = serialize(http::Request {
        method: "GET".to_string(),
        path: "/test".to_string(),
        query_string: "".to_string(),
        header: HashMap::new(),
        body: serialize(&data)?, // this is msgpack encoding inside an HTTP body.
    })?;

    let now = Instant::now();
    for _i in 0..10_000 {
        h.call_actor(&actor_id, "HandleRequest", &payload).await?;
    }
    let done = now.elapsed().as_secs();
    println!(
        "10,000 iterations took {}s, {}/iteration",
        done,
        done as f64 / 10_000 as f64
    );

    Ok(())
}

// Copy this struct into whatever benchmark test is invoking this actor
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Data {
    pub field1: f64,
    pub field2: u32,
    pub field3: String,
}

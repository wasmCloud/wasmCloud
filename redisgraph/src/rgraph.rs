use redisgraph::Graph;
use std::error::Error;
use wasmcloud_actor_core::CapabilityConfiguration;

const ENV_REDIS_URL: &str = "URL";

pub(crate) fn initialize_client(
    config: CapabilityConfiguration,
) -> Result<redis::Client, Box<dyn Error>> {
    let redis_url = match config.values.get(ENV_REDIS_URL) {
        Some(v) => v,
        None => "redis://0.0.0.0:6379/",
    }
    .to_string();

    info!(
        "Attempting to connect {} to Redis(graph) at {}",
        config.module, redis_url
    );
    match redis::Client::open(redis_url.as_ref()) {
        Ok(c) => Ok(c),
        Err(e) => Err(format!("Failed to connect to Redis(graph): {}", e).into()),
    }
}

pub(crate) fn open_graph(
    connection: redis::Connection,
    graph_name: &str,
) -> Result<Graph, Box<dyn Error>> {
    match Graph::open(connection, graph_name.to_string()) {
        // Invokes a dummy node create and a dummy node to delete, ensures graph exists
        Ok(g) => Ok(g),
        Err(e) => Err(format!("Could not open graph: {:?}", e).into()),
    }
}

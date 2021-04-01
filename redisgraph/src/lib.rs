//! # RedisGraph implementation of the wasmCloud Graph Database Capability Provider API
//!
//! Provides an implementation of the wasmcloud:graphdb contract for RedisGraph
//! using the Cypher language

use codec::{
    capabilities::{CapabilityProvider, Dispatcher, NullDispatcher},
    capability_provider,
    core::{OP_BIND_ACTOR, OP_REMOVE_ACTOR},
};
use log::{trace, warn};
use redis::{Connection, RedisResult};
use redisgraph::{Graph, RedisGraphResult, ResultSet};
use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, RwLock},
};
use wasmcloud_actor_core::CapabilityConfiguration;
use wasmcloud_actor_graphdb::{
    deserialize, serialize, DeleteGraphArgs, DeleteResponse, QueryGraphArgs, QueryResponse,
    OP_DELETE, OP_QUERY,
};
use wasmcloud_provider_core as codec;
mod conversions;
mod rgraph;

#[allow(unused)]
const CAPABILITY_ID: &str = "wasmcloud:graphdb";
const SYSTEM_ACTOR: &str = "system";

type GraphHandlerResult = Result<Vec<u8>, Box<dyn Error + Send + Sync + 'static>>;

// Enable the static_plugin feature in your Cargo.toml if you want to statically
// embed this capability instead of loading the dynamic library at runtime.

#[cfg(not(feature = "static_plugin"))]
capability_provider!(RedisgraphProvider, RedisgraphProvider::new);

#[derive(Clone)]
pub struct RedisgraphProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    clients: Arc<RwLock<HashMap<String, redis::Client>>>,
}

impl Default for RedisgraphProvider {
    fn default() -> Self {
        let _ = env_logger::builder().format_module_path(false).try_init();

        RedisgraphProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl RedisgraphProvider {
    pub fn new() -> Self {
        Self::default()
    }

    // Handles a request to query a graph, passing the query on to the RedisGraph client
    fn query_graph(&self, actor: &str, query: QueryGraphArgs) -> GraphHandlerResult {
        let mut g = self
            .open_graph(actor, &query.graph_name)
            .map_err(|e| format!("{}", e))?;
        let rs: RedisGraphResult<ResultSet> = g.query(&query.query);
        match rs {
            Ok(rs) => {
                let response = QueryResponse {
                    result_set: to_common_resultset(rs)?,
                };
                Ok(serialize(response)?)
            }
            Err(e) => Err(format!("Graph query failure: {:?}", e).into()),
        }
    }

    // Handles a request to delete a graph
    fn delete_graph(&self, actor: &str, delete: DeleteGraphArgs) -> GraphHandlerResult {
        let g = self
            .open_graph(actor, &delete.graph_name)
            .map_err(|e| format!("{}", e))?; // Ensure Graph exists
        let rs: RedisGraphResult<()> = g.delete();
        match rs {
            Ok(_) => Ok(serialize(DeleteResponse { success: true })?),
            Err(e) => Err(format!("Failed to delete graph: {:?}", e).into()),
        }
    }

    // Called when a previously bound actor is removed from the host. This allows
    // us to clean up resources (drop the client) used by the actor
    fn deconfigure(&self, actor: &str) -> GraphHandlerResult {
        if self.clients.write().unwrap().remove(actor).is_none() {
            warn!("Attempted to de-configure non-existent actor: {}", actor);
        }
        Ok(vec![])
    }

    // Called when an actor is bound to this capability provider by the host
    // We create a Redis client in response to this message
    fn configure(&self, config: CapabilityConfiguration) -> GraphHandlerResult {
        trace!("Configuring provider for {}", &config.module);
        let c = rgraph::initialize_client(config.clone()).map_err(|e| format!("{}", e))?;

        self.clients.write().unwrap().insert(config.module, c);
        Ok(vec![])
    }

    fn actor_con(&self, actor: &str) -> RedisResult<Connection> {
        let lock = self.clients.read().unwrap();
        if let Some(client) = lock.get(actor) {
            client.get_connection()
        } else {
            Err(redis::RedisError::from((
                redis::ErrorKind::InvalidClientConfig,
                "No client for this actor. Did the host configure it?",
            )))
        }
    }

    fn open_graph(&self, actor: &str, graph: &str) -> Result<Graph, Box<dyn Error>> {
        let conn = self.actor_con(actor)?;
        let g = rgraph::open_graph(conn, &graph)?;
        Ok(g)
    }
}

// Convert the redisgraph::ResultSet type to the common actor_graphdb::ResultSet
// type. This ensures guests using the actor_graphdb library (e.g. Actors) can
// interact with the common ResultSet type.
// WARNING: this could fail if redisgraph is upgraded and changes the shape of its
// ResultSet type
fn to_common_resultset(
    rs: redisgraph::ResultSet,
) -> Result<wasmcloud_actor_graphdb::ResultSet, Box<dyn Error + Send + Sync>> {
    let columns = rs
        .columns
        .into_iter()
        .map(conversions::redisgraph_column_to_common)
        .collect::<Vec<_>>();
    let statistics = rs.statistics.0;
    Ok(wasmcloud_actor_graphdb::ResultSet {
        columns,
        statistics,
    })
}

impl CapabilityProvider for RedisgraphProvider {
    // Invoked by the runtime host to give this provider plugin the ability to communicate
    // with actors
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        trace!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    // Invoked by host runtime to allow an actor to make use of the capability
    // All providers MUST handle the OP_BIND_ACTOR and OP_REMOVE_ACTOR messages, even
    // if no resources are provisioned or cleaned up
    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => self.configure(deserialize(msg)?),
            OP_QUERY => self.query_graph(actor, deserialize(msg)?),
            OP_DELETE => self.delete_graph(actor, deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => self.deconfigure(actor),
            _ => Err("bad dispatch".into()),
        }
    }

    fn stop(&self) {}
}

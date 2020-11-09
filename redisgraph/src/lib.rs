//! # RedisGraph implementation of the waSCC Graph Database Capability Provider API
//!
//! Provides an implementation of the wascc:graphdb contract for RedisGraph
//! using the Cypher language

#[macro_use]
extern crate wascc_codec as codec;

extern crate wasccgraph_common as common;

#[macro_use]
extern crate log;

use codec::capabilities::{
    CapabilityDescriptor, CapabilityProvider, Dispatcher, NullDispatcher, OperationDirection,
    OP_GET_CAPABILITY_DESCRIPTOR,
};
use codec::core::{CapabilityConfiguration, OP_BIND_ACTOR, OP_REMOVE_ACTOR};
use codec::{deserialize, serialize};

use std::error::Error;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use common::protocol::*;
use redis::Connection;
use redis::RedisResult;
use redisgraph::{Graph, RedisGraphResult, ResultSet};

mod rgraph;

const CAPABILITY_ID: &str = "wascc:graphdb";
const SYSTEM_ACTOR: &str = "system";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REVISION: u32 = 2; // Increment for each crates publish

// Enable the static_plugin feature in your Cargo.toml if you want to statically
// embed this capability instead of loading the dynamic library at runtime.

#[cfg(not(feature = "static_plugin"))]
capability_provider!(WasccRedisgraphProvider, WasccRedisgraphProvider::new);

pub struct WasccRedisgraphProvider {
    dispatcher: RwLock<Box<dyn Dispatcher>>,
    clients: Arc<RwLock<HashMap<String, redis::Client>>>,
}

impl Default for WasccRedisgraphProvider {
    fn default() -> Self {
        let _ = env_logger::builder().format_module_path(false).try_init();

        WasccRedisgraphProvider {
            dispatcher: RwLock::new(Box::new(NullDispatcher::new())),
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl WasccRedisgraphProvider {
    pub fn new() -> Self {
        Self::default()
    }

    // Handles a request to query a graph, passing the query on to the RedisGraph client
    fn query_graph(&self, actor: &str, query: QueryRequest) -> Result<Vec<u8>, Box<dyn Error>> {
        trace!("Querying graph database: {:?}", query);
        let mut g = self.open_graph(actor, &query.graph_name)?;
        let rs: RedisGraphResult<ResultSet> = g.query(&query.query);
        match rs {
            Ok(rs) => Ok(serialize(&to_common_resultset(rs)?)?),
            Err(e) => Err(format!("Graph query failure: {:?}", e).into()),
        }
    }

    // Handles a request to delete a graph
    fn delete_graph(&self, actor: &str, delete: DeleteRequest) -> Result<Vec<u8>, Box<dyn Error>> {
        let g = self.open_graph(actor, &delete.graph_name)?; // Ensure Graph exists
        let rs: RedisGraphResult<()> = g.delete();
        match rs {
            Ok(_) => Ok(vec![]),
            Err(e) => Err(format!("Failed to delete graph: {:?}", e).into()),
        }
    }

    // Called when a previously bound actor is removed from the host. This allows
    // us to clean up resources (drop the client) used by the actor
    fn deconfigure(&self, actor: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        if self.clients.write().unwrap().remove(actor).is_none() {
            warn!("Attempted to de-configure non-existent actor: {}", actor);
        }
        Ok(vec![])
    }

    // Called when an actor is bound to this capability provider by the host
    // We create a Redis client in response to this message
    fn configure(&self, config: CapabilityConfiguration) -> Result<Vec<u8>, Box<dyn Error>> {
        trace!("Configuring provider for {}", &config.module);
        let c = rgraph::initialize_client(config.clone())?;

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

    fn get_descriptor(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(serialize(
            CapabilityDescriptor::builder()
                .id(CAPABILITY_ID)
                .name("waSCC Graph Database Provider (RedisGraph)")
                .long_description("A capability provider exposing Cypher-based RedisGraph database access to waSCC actors")
                .version(VERSION)
                .revision(REVISION)
                .with_operation(
                    OP_QUERY,
                    OperationDirection::ToProvider,
                    "Executes a Cypher query against the database and returns the results"
                )
                .with_operation(
                    OP_DELETE,
                    OperationDirection::ToProvider,
                    "Deletes a graph database"
                )
                .build()
        )?)
    }
}

// Force a serialization trip between the internal redisgraph::ResultSet type and
// the shared common protocol ResultSet type. If this works, then we should be
// reasonably confident the guest graph library can unpack this within the actor
// WARNING: this could fail if redisgraph is upgraded and changes the shape of its
// ResultSet type
fn to_common_resultset(rs: redisgraph::ResultSet) -> Result<common::ResultSet, Box<dyn Error>> {
    let input = serialize(&rs)?;
    let output: common::ResultSet = deserialize(&input)?;
    Ok(output)
}

impl CapabilityProvider for WasccRedisgraphProvider {
    // Invoked by the runtime host to give this provider plugin the ability to communicate
    // with actors
    fn configure_dispatch(&self, dispatcher: Box<dyn Dispatcher>) -> Result<(), Box<dyn Error>> {
        trace!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    // Invoked by host runtime to allow an actor to make use of the capability
    // All providers MUST handle the OP_BIND_ACTOR and OP_REMOVE_ACTOR messages, even
    // if no resources are provisioned or cleaned up
    fn handle_call(&self, actor: &str, op: &str, msg: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => self.configure(deserialize(msg)?),
            OP_QUERY => self.query_graph(actor, deserialize(msg)?),
            OP_DELETE => self.delete_graph(actor, deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => self.deconfigure(actor),
            OP_GET_CAPABILITY_DESCRIPTOR if actor == SYSTEM_ACTOR => self.get_descriptor(),
            _ => Err("bad dispatch".into()),
        }
    }
}

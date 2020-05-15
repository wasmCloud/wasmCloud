//! # Common protocol types
//!
//! Used to describe the communication between graphdb actor and graphdb capability
//! provider.

/// An actor sends a query request (via ergonomic API wrapper) to the capability provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub query: String,
    pub graph_name: String,
}

/// A request to delete a graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteRequest {
    pub graph_name: String,
}

/// The operation to request a query of graph data
pub const OP_QUERY: &str = "QueryGraph";
/// The operation to request the deletion of a graph
pub const OP_DELETE: &str = "DeleteGraph";

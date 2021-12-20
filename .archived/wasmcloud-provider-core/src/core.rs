//! # Core Constants
//!
//! This module contains a few constants that are common enough that they
//! can be shared here. Do not use this to share RPC data types

pub const OP_PERFORM_LIVE_UPDATE: &str = "PerformLiveUpdate";
pub const OP_BIND_ACTOR: &str = "BindActor";
pub const OP_REMOVE_ACTOR: &str = "RemoveActor";
pub const OP_HEALTH_REQUEST: &str = "HealthRequest";

pub const SYSTEM_ACTOR: &str = "system";

// Keys used for providing actor claim data to a capability provider during binding

pub const CONFIG_WASMCLOUD_CLAIMS_ISSUER: &str = "__wasmcloud_issuer";
pub const CONFIG_WASMCLOUD_CLAIMS_CAPABILITIES: &str = "__wasmcloud_capabilities";
pub const CONFIG_WASMCLOUD_CLAIMS_NAME: &str = "__wasmcloud_name";
pub const CONFIG_WASMCLOUD_CLAIMS_EXPIRES: &str = "__wasmcloud_expires";
pub const CONFIG_WASMCLOUD_CLAIMS_TAGS: &str = "__wasmcloud_tags";

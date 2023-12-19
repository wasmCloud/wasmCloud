//! A set of standard names for capabilities that can be provided by a host

use std::collections::HashMap;
use std::sync::OnceLock;

pub const BLOB: &str = "wasmcloud:blobstore";
pub const HTTP_CLIENT: &str = "wasmcloud:httpclient";
pub const HTTP_SERVER: &str = "wasmcloud:httpserver";
pub const KEY_VALUE: &str = "wasmcloud:keyvalue";
pub const MESSAGING: &str = "wasmcloud:messaging";
pub const EVENTSTREAMS: &str = "wasmcloud:eventstreams";
pub const NUMBERGEN: &str = "wasmcloud:builtin:numbergen";
pub const LOGGING: &str = "wasmcloud:builtin:logging";
pub const LATTICE_CONTROL: &str = "wasmcloud:latticecontrol";

static CAPABILITY_NAMES: OnceLock<HashMap<&str, &str>> = OnceLock::new();

fn get_capability_names() -> &'static HashMap<&'static str, &'static str> {
    CAPABILITY_NAMES.get_or_init(|| {
        HashMap::from([
            (MESSAGING, "Messaging"),
            (KEY_VALUE, "K/V Store"),
            (HTTP_SERVER, "HTTP Server"),
            (HTTP_CLIENT, "HTTP Client"),
            (BLOB, "Blob Store"),
            (EVENTSTREAMS, "Event Streams"),
            (NUMBERGEN, "Number Generation"),
            (LATTICE_CONTROL, "Lattice control"),
            (LOGGING, "Logging"),
        ])
    })
}

#[must_use]
pub fn capability_name(cap: &str) -> String {
    get_capability_names()
        .get(cap)
        .map_or(cap.to_string(), |item| (*item).to_string())
}

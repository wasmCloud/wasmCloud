//! A set of standard names for capabilities that can be provided by a host
pub const BLOB: &str = "wasmcloud:blobstore";
pub const HTTP_CLIENT: &str = "wasmcloud:httpclient";
pub const HTTP_SERVER: &str = "wasmcloud:httpserver";
pub const KEY_VALUE: &str = "wasmcloud:keyvalue";
pub const MESSAGING: &str = "wasmcloud:messaging";
pub const EVENTSTREAMS: &str = "wasmcloud:eventstreams";
pub const NUMBERGEN: &str = "wasmcloud:builtin:numbergen";
pub const LOGGING: &str = "wasmcloud:builtin:logging";

use std::collections::HashMap;

lazy_static! {
    static ref CAPABILITY_NAMES: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert(MESSAGING, "Messaging");
        m.insert(KEY_VALUE, "K/V Store");
        m.insert(HTTP_SERVER, "HTTP Server");
        m.insert(HTTP_CLIENT, "HTTP Client");
        m.insert(BLOB, "Blob Store");
        m.insert(EVENTSTREAMS, "Event Streams");
        m.insert(NUMBERGEN, "Number Generation");
        m.insert(LOGGING, "Logging");
        m
    };
}

pub fn capability_name(cap: &str) -> String {
    CAPABILITY_NAMES
        .get(cap)
        .map_or(cap.to_string(), |item| item.to_string())
}

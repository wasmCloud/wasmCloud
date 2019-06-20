//! A set of standard names for capabilities that can be provided by a host
pub const MESSAGING: &'static str = "wascap:messaging";
pub const KEY_VALUE: &'static str = "wascap:keyvalue";
pub const HTTP_SERVER: &'static str = "wascap:http_server";
pub const HTTP_CLIENT: &'static str = "wascap:http_client";
pub const LOGGING: &'static str = "wascap:logging";
pub const BLOB: &'static str = "wascap:blobstore";

use std::collections::HashMap;

lazy_static! {
    static ref CAPABILITY_NAMES: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert(MESSAGING, "Messaging");
        m.insert(KEY_VALUE, "K/V Store");
        m.insert(LOGGING, "Logging");
        m.insert(HTTP_SERVER, "HTTP Server");
        m.insert(HTTP_CLIENT, "HTTP Client");
        m
    };
}

pub fn capability_name(cap: &str) -> String {
    CAPABILITY_NAMES
        .get(cap)
        .map_or(cap.to_string(), |item| item.to_string())
}

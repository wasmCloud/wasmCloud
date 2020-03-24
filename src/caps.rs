// Copyright 2015-2019 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! A set of standard names for capabilities that can be provided by a host
pub const MESSAGING: &str = "wascc:messaging";
pub const KEY_VALUE: &str = "wascc:keyvalue";
pub const HTTP_SERVER: &str = "wascc:http_server";
pub const HTTP_CLIENT: &str = "wascc:http_client";
pub const BLOB: &str = "wascc:blobstore";
pub const EVENTSTREAMS: &str = "wascc:eventstreams";
pub const EXTRAS: &str = "wascc:extras";
pub const LOGGING: &str = "wascc:logging";

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
        m.insert(EXTRAS, "Extras");
        m.insert(LOGGING, "Logging");
        m
    };
}

pub fn capability_name(cap: &str) -> String {
    CAPABILITY_NAMES
        .get(cap)
        .map_or(cap.to_string(), |item| item.to_string())
}

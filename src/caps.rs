// Copyright 2015-2018 Capital One Services, LLC
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

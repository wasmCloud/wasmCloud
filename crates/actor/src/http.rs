// TODO: This should be defined in `compat` module only

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Request contains data sent to actor about the http request
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Request {
    /// HTTP method. One of: GET,POST,PUT,DELETE,HEAD,OPTIONS,CONNECT,PATCH,TRACE
    #[serde(default)]
    pub method: String,
    /// Full request path
    #[serde(default)]
    pub path: String,
    /// Query string
    #[serde(rename = "queryString")]
    #[serde(default)]
    pub query_string: String,
    /// Map of request headers (string key, string value)
    pub header: HashMap<String, Vec<String>>,
    /// Request body as a byte array. May be empty.
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

/// Response contains the actor's response to return to the http client
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Response {
    /// Three-digit number, usually in the range 100-599,
    /// A value of 200 indicates success.
    #[serde(rename = "statusCode")]
    #[serde(default)]
    pub status_code: u16,
    /// Map of headers (string keys, list of values)
    pub header: HashMap<String, Vec<String>>,
    /// Body of response as a byte array. May be an empty array.
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

impl Default for Response {
    fn default() -> Response {
        Self {
            status_code: 200,
            body: Vec::default(),
            header: HashMap::default(),
        }
    }
}

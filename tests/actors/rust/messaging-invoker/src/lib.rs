use anyhow::{anyhow, Context as _, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

wit_bindgen::generate!({
    world: "messaging-invoker",
    additional_derives: [serde::Serialize, serde::Deserialize],
});

/// The struct which will hold implementations of the subscriber
struct Actor;

use wasi::logging::logging;
use wasmcloud::bus::lattice;
use wasmcloud::keyvalue::key_value;
use wasmcloud::messaging::consumer::publish;
use wasmcloud::messaging::types;

/// Logging context used for grouping messages together when passed through wasi:logging
const LOG_CTX: &str = "messaging-invoker";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
/// Messages sent over the messaging provider to trigger an invocation
struct InvokeRequest {
    /// Link name over which to invoke the request
    /// (it's assumed that the link to this actor has been made prior to invocation)
    link_name: String,

    /// WIT namespace to invoke
    wit_ns: String,

    /// WIT package to invoke
    wit_pkg: String,

    /// WIT interface to invoke
    wit_iface: String,

    /// WIT data to invoke
    wit_fn: String,

    /// Data to use for the invocation, which must be of a known
    /// form that this actor can interpret, as base64 encoded JSON
    params_json_b64: Vec<String>,
}

impl exports::wasmcloud::messaging::handler::Guest for Actor {
    fn handle_message(msg: types::BrokerMessage) -> Result<(), String> {
        logging::log(
            logging::Level::Debug,
            LOG_CTX,
            format!("received message: {msg:?}").as_str(),
        );

        // Decode the invoke request that should be coming across the wire
        let InvokeRequest {
            link_name,
            wit_ns,
            wit_pkg,
            wit_iface,
            wit_fn,
            params_json_b64,
        } = match serde_json::from_slice(&msg.body) {
            Ok(v) => v,
            Err(e) => {
                return Err(format!(
                    "failed to decode payload into invocation request: {e}"
                ))
            }
        };

        // Rebuild the complete operation
        let operation = format!("{wit_ns}:{wit_pkg}/{wit_iface}.{wit_fn}");

        // Decode params coming in as b64
        let params_json: Vec<Bytes> = params_json_b64
            .iter()
            .flat_map(|v| STANDARD.decode(v))
            .map(Bytes::from)
            .collect::<Vec<Bytes>>();
        if params_json_b64.len() != params_json.len() {
            return Err(format!("failed to decode one or more base64 encoded JSON params while executing operation [{operation}]"));
        }

        // Set the intended link name before invocation, if necessary
        if link_name != lattice::get_link_name() {
            logging::log(
                logging::Level::Debug,
                LOG_CTX,
                format!("detected alternate link name [{link_name}]").as_str(),
            );
            lattice::set_link_name(
                &link_name,
                vec![lattice::CallTargetInterface::new(
                    &wit_ns,
                    &wit_pkg,
                    &wit_iface,
                    Some(&wit_fn),
                )],
            );
        }

        logging::log(
            logging::Level::Debug,
            LOG_CTX,
            format!("about to handle [{operation}]").as_str(),
        );

        // Translate the incoming invocation request to a static invocation
        let v = handle_operation(&operation, params_json)
            .with_context(|| format!("failed to handle operation [{operation}]"))
            .map_err(|err| format!("{err:#}"))?;
        if let Some(resp_subject) = msg.reply_to {
            publish(&types::BrokerMessage {
                subject: resp_subject,
                reply_to: None,
                body: v.unwrap_or_default().into(),
            })?;
        }
        Ok(())
    }
}

/// Helper method for deserializing JSON contents
pub fn json_deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T> {
    serde_json::from_slice(buf).map_err(|e| anyhow!(e))
}

/// Helper method for serializing contents
pub fn json_serialize<T: Serialize>(data: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(data).map_err(|e| anyhow!(e))
}

/// Handle an operation
fn handle_operation(operation: &str, mut params_json: Vec<Bytes>) -> Result<Option<Bytes>> {
    logging::log(
        logging::Level::Debug,
        LOG_CTX,
        format!("performing [{operation}]").as_str(),
    );

    match operation {
        "wasmcloud:keyvalue/key-value.get" => {
            let key: String =
                json_deserialize(&params_json.pop().with_context(|| {
                    format!("failed to read param for operation [{operation}]")
                })?)
                .with_context(|| format!("failed to parse key from [{operation}]"))?;

            let value = key_value::get(&key);
            Ok(Some(Bytes::from(json_serialize(&value).with_context(
                || format!("failed to serialize results for operation [{operation}]"),
            )?)))
        }

        "wasmcloud:keyvalue/key-value.set" => {
            let req =
                json_deserialize::<key_value::SetRequest>(&params_json.pop().with_context(
                    || format!("failed to read param for operation [{operation}]"),
                )?)
                .with_context(|| format!("failed to parse SetRequest from [{operation}]"))?;
            key_value::set(&req);
            Ok(None)
        }
        // todo(vados-cosmonic): more invoking
        // "wasmcloud:blobstore/blobstore.get" => todo!(),
        // "wasmcloud:lattice-control/lattice-controller.something" => todo!(),
        // "test-actors:testing/busybox.something" => todo!(),
        _ => {
            logging::log(
                logging::Level::Error,
                LOG_CTX,
                format!(
                    "unrecognized {} param function [{operation}]",
                    params_json.len()
                )
                .as_str(),
            );
            Ok(None)
        }
    }
}

export!(Actor);

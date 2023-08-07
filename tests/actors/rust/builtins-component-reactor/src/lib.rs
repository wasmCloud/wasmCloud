wit_bindgen::generate!("actor");

use std::io::{stdin, stdout, Read, Write};

use serde::Deserialize;
use serde_json::json;
use wasmcloud_actor::wasi::keyvalue;
use wasmcloud_actor::wasi::logging::logging;
use wasmcloud_actor::wasi::random::random;
use wasmcloud_actor::wasmcloud::messaging;
use wasmcloud_actor::{
    debug, error, info, trace, warn, HostRng, HttpRequest, HttpResponse, InputStreamReader,
    OutputStreamWriter,
};

struct Actor;

impl exports::wasmcloud::bus::guest::Guest for Actor {
    fn call(operation: String) -> Result<(), String> {
        assert_eq!(operation, "HttpServer.HandleRequest");
        let HttpRequest {
            method,
            path,
            query_string,
            header: _,
            body,
        } = rmp_serde::from_read(stdin()).expect("failed to read request");
        assert_eq!(method, "POST");
        assert_eq!(path, "/");
        assert_eq!(query_string, "");
        // TODO: Validate headers

        #[derive(Deserialize)]
        struct Request {
            min: u32,
            max: u32,
        }
        let Request { min, max } =
            serde_json::from_slice(&body).expect("failed to decode request body");

        logging::log(logging::Level::Trace, "trace-context", "trace");
        logging::log(logging::Level::Debug, "debug-context", "debug");
        logging::log(logging::Level::Info, "info-context", "info");
        logging::log(logging::Level::Warn, "warn-context", "warn");
        logging::log(logging::Level::Error, "error-context", "error");

        trace!(context: "trace-context", "trace");
        debug!(context: "debug-context", "debug");
        info!(context: "info-context", "info");
        warn!(context: "warn-context", "warn");
        error!(context: "error-context", "error");

        trace!("trace");
        debug!("debug");
        info!("info");
        warn!("warn");
        error!("error");

        let res = json!({
            "get_random_bytes": random::get_random_bytes(8),
            "get_random_u64": random::get_random_u64(),
            "guid": HostRng::generate_guid(),
            "random_32": HostRng::random32(),
            "random_in_range": HostRng::random_in_range(min, max),
        });
        eprintln!("response: `{res:?}`");
        let body = serde_json::to_vec(&res).expect("failed to encode response to JSON");
        let res = rmp_serde::to_vec(&HttpResponse {
            body: body.clone(),
            ..Default::default()
        })
        .expect("failed to serialize response");
        let mut stdout = stdout();
        stdout
            .lock()
            .write_all(&res)
            .expect("failed to write response");
        stdout.flush().expect("failed to flush stdout");
        messaging::consumer::publish(&messaging::types::BrokerMessage {
            body: Some(body.clone()),
            reply_to: None,
            subject: "test".into(),
        })
        .expect("failed to publish response");

        let foo_key = String::from("foo");
        let bucket = keyvalue::types::open_bucket("")
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to open empty bucket");
        keyvalue::readwrite::exists(bucket, &foo_key)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to check whether `foo` exists")
            .then_some(())
            .expect("`foo` does not exist");

        let foo_value = keyvalue::readwrite::get(bucket, &foo_key)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to get `foo`");

        let size = keyvalue::types::size(foo_value);
        assert_eq!(size, 3);

        let foo_value = keyvalue::types::incoming_value_consume_sync(foo_value)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to get incoming value buffer");
        assert_eq!(foo_value, b"bar");

        let foo_value = keyvalue::readwrite::get(bucket, &foo_key)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to get `foo`");
        let foo_stream = keyvalue::types::incoming_value_consume_async(foo_value)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to get incoming value stream");
        let mut foo_value = vec![];
        let n = InputStreamReader::from(foo_stream)
            .read_to_end(&mut foo_value)
            .expect("failed to read value from keyvalue input stream");
        assert_eq!(n, 3);
        assert_eq!(foo_value, b"bar");

        keyvalue::readwrite::delete(bucket, &foo_key)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to delete `foo`");

        // NOTE: If https://github.com/WebAssembly/wasi-keyvalue/pull/18 is merged, this should not
        // return an error
        keyvalue::readwrite::exists(bucket, &foo_key)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect_err(
                "`exists` method should have returned an error for `foo` key, which was deleted",
            );

        let result_key = String::from("result");
        let result_value = keyvalue::types::new_outgoing_value();
        let result_stream = keyvalue::types::outgoing_value_write_body(result_value)
            .expect("failed to get outgoing value output stream");
        let n = OutputStreamWriter::from(result_stream)
            .write(&body)
            .expect("failed to write result to keyvalue output stream");
        assert_eq!(n, body.len());
        keyvalue::readwrite::set(bucket, &result_key, result_value)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to set `result`");
        Ok(())
    }
}

export_actor!(Actor);

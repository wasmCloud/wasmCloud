wit_bindgen::generate!("actor");

use std::io::{stdin, stdout, Write};

use serde::Deserialize;
use serde_json::json;
use wasmcloud_actor::wasi::logging::logging;
use wasmcloud_actor::wasi::random::random;
use wasmcloud_actor::wasmcloud::bus;
use wasmcloud_actor::wasmcloud::bus::lattice::TargetEntity;
use wasmcloud_actor::{debug, error, info, trace, warn, HostRng, HttpRequest, HttpResponse};
use wasmcloud_actor::{keyvalue, messaging};

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
        let body = serde_json::to_string(&res).expect("failed to encode response to JSON");
        let res = rmp_serde::to_vec(&HttpResponse {
            body: body.clone().into(),
            ..Default::default()
        })
        .expect("failed to serialize response");
        let mut stdout = stdout();
        stdout
            .lock()
            .write_all(&res)
            .expect("failed to write response");
        stdout.flush().expect("failed to flush stdout");

        // TODO: Use the "component-like" wrappers

        let messaging_target = TargetEntity::Link(Some("messaging".into()));
        bus::lattice::set_target(
            Some(&messaging_target),
            &[bus::lattice::target_wasmcloud_messaging_consumer()],
        );
        let buf = rmp_serde::to_vec_named(&messaging::PubMessage {
            body: body.clone().into(),
            reply_to: Some("noreply".into()),
            subject: "test-messaging-publish".into(),
        })
        .expect("failed to encode `PubMessage`");
        bus::host::call_sync(
            Some(&messaging_target),
            "wasmcloud:messaging/Messaging.Publish",
            &buf,
        )
        .expect("failed to publish response");

        let buf = rmp_serde::to_vec_named(&messaging::RequestMessage {
            subject: "test-messaging-request".into(),
            body: b"foo".to_vec(),
            timeout_ms: 1000,
        })
        .expect("failed to encode `RequestMessage`");
        let buf = bus::host::call_sync(
            Some(&messaging_target),
            "wasmcloud:messaging/Messaging.Request",
            &buf,
        )
        .expect("failed to request response");
        let messaging::ReplyMessage {
            body: response_body,
            reply_to,
            subject: _,
        } = rmp_serde::from_slice(&buf).expect("failed to decode `ReplyMessage`");
        assert_eq!(response_body.as_slice(), b"bar");
        assert_eq!(reply_to, None);

        let buf = rmp_serde::to_vec_named(&messaging::RequestMessage {
            subject: "test-messaging-request-multi".into(),
            body: b"foo".to_vec(),
            timeout_ms: 1000,
        })
        .expect("failed to encode `RequestMessage`");
        let buf = bus::host::call_sync(
            Some(&messaging_target),
            "wasmcloud:messaging/Messaging.Request",
            &buf,
        )
        .expect("failed to request response");
        let messaging::ReplyMessage {
            body: response_body,
            reply_to,
            subject: _,
        } = rmp_serde::from_slice(&buf).expect("failed to decode `ReplyMessage`");
        assert_eq!(response_body.as_slice(), b"bar");
        assert_eq!(reply_to, None);

        let keyvalue_target = TargetEntity::Link(Some("keyvalue".into()));
        bus::lattice::set_target(
            Some(&keyvalue_target),
            &[bus::lattice::target_wasi_keyvalue_readwrite()],
        );

        let foo_key = String::from("foo");

        let buf = rmp_serde::to_vec_named(&foo_key).expect("failed to encode string");
        let buf = bus::host::call_sync(
            Some(&keyvalue_target),
            "wasmcloud:keyvalue/KeyValue.Contains",
            &buf,
        )
        .expect("failed to check if key exists");
        rmp_serde::from_slice::<bool>(&buf)
            .expect("failed to decode boolean")
            .then_some(())
            .expect("`foo` does not exist");

        let buf = rmp_serde::to_vec_named(&foo_key).expect("failed to encode string");
        let buf = bus::host::call_sync(
            Some(&keyvalue_target),
            "wasmcloud:keyvalue/KeyValue.Get",
            &buf,
        )
        .expect("failed to get `foo`");
        let keyvalue::GetResponse { value, exists } =
            rmp_serde::from_slice(&buf).expect("failed to decode `Get` response");
        assert!(exists);
        assert_eq!(value, "bar");

        let buf = rmp_serde::to_vec_named(&foo_key).expect("failed to encode string");
        let buf = bus::host::call_sync(
            Some(&keyvalue_target),
            "wasmcloud:keyvalue/KeyValue.Del",
            &buf,
        )
        .expect("failed to delete `foo`");
        rmp_serde::from_slice::<bool>(&buf)
            .expect("failed to decode boolean")
            .then_some(())
            .expect("`foo` did not exist");

        let buf = rmp_serde::to_vec_named(&keyvalue::SetRequest {
            key: "result".into(),
            value: body.clone(),
            expires: 0,
        })
        .expect("failed to encode `SetRequest`");
        bus::host::call_sync(
            Some(&keyvalue_target),
            "wasmcloud:keyvalue/KeyValue.Set",
            &buf,
        )
        .expect("failed to set `result`");

        Ok(())
    }
}

export_actor!(Actor);

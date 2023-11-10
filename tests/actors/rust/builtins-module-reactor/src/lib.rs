use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
use serde_json::json;
use wasmcloud_actor::wasi::logging::logging;
use wasmcloud_actor::wasi::random::random;
use wasmcloud_actor::wasmcloud::bus;
use wasmcloud_actor::wasmcloud::bus::lattice::TargetEntity;
use wasmcloud_actor::{
    debug, error, export_actor, http, info, trace, warn, HostRng, HttpHandler, HttpResponse,
    HttpServerRequest,
};
use wasmcloud_actor::{keyvalue, messaging};

#[derive(Default)]
struct HttpLogRng;

impl HttpHandler for HttpLogRng {
    fn handle_request(
        &self,
        HttpServerRequest {
            method,
            path,
            query_string,
            header,
            body,
        }: HttpServerRequest,
    ) -> Result<HttpResponse, String> {
        #[derive(Deserialize)]
        struct Request {
            min: u32,
            max: u32,
            port: u16,
        }

        assert_eq!(method, "POST");
        assert_eq!(path, "/foo");
        assert_eq!(query_string, "bar=baz");

        let mut header_iter = header.into_iter().collect::<BTreeMap<_, _>>().into_iter();
        assert_eq!(
            header_iter.next(),
            Some(("accept".into(), vec!["*/*".into()]))
        );
        let (content_length_key, _) = header_iter.next().expect("`content-length` header missing");
        assert_eq!(content_length_key, "content-length");
        let (host_key, _) = header_iter.next().expect("`host` header missing");
        assert_eq!(host_key, "host");
        assert_eq!(
            header_iter.next(),
            Some(("test-header".into(), vec!["test-value".into()]))
        );
        assert!(header_iter.next().is_none());

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

        let Request { min, max, port } =
            serde_json::from_slice(&body).expect("failed to decode request body");

        let res = json!({
            "get_random_bytes": random::get_random_bytes(8),
            "get_random_u64": random::get_random_u64(),
            "guid": HostRng::generate_guid(),
            "random_32": HostRng::random32(),
            "random_in_range": HostRng::random_in_range(min, max),
            "long_value": "1234567890".repeat(1000),
            "config_value": [],
            "all_config": [],
        });
        eprintln!("response: `{res:?}`");
        let body = serde_json::to_string(&res).expect("failed to encode response to JSON");

        // TODO: Use the "component-like" wrappers

        let messaging_target = TargetEntity::Link(Some("messaging".into()));
        bus::lattice::set_target(
            Some(&messaging_target),
            vec![bus::lattice::TargetInterface::wasmcloud_messaging_consumer()],
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
            vec![bus::lattice::TargetInterface::wasi_keyvalue_readwrite()],
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

        bus::lattice::set_target(
            Some(&keyvalue_target),
            vec![bus::lattice::TargetInterface::wasi_keyvalue_atomic()],
        );

        let counter_key = String::from("counter");

        let buf = rmp_serde::to_vec_named(&keyvalue::IncrementRequest {
            key: counter_key.clone(),
            value: 1,
        })
        .expect("failed to encode `IncrementRequest`");
        let buf = bus::host::call_sync(
            Some(&keyvalue_target),
            "wasmcloud:keyvalue/KeyValue.Increment",
            &buf,
        )
        .expect("failed to increment `counter`");
        let value: i32 =
            rmp_serde::from_slice(&buf).expect("failed to decode `Increment` response");
        assert_eq!(value, 1);

        let buf = rmp_serde::to_vec_named(&keyvalue::IncrementRequest {
            key: counter_key.clone(),
            value: 41,
        })
        .expect("failed to encode `IncrementRequest`");
        let buf = bus::host::call_sync(
            Some(&keyvalue_target),
            "wasmcloud:keyvalue/KeyValue.Increment",
            &buf,
        )
        .expect("failed to increment `counter`");
        let value: i32 =
            rmp_serde::from_slice(&buf).expect("failed to decode `Increment` response");
        assert_eq!(value, 42);

        // TODO: Use blobstore

        bus::host::call_sync(
            Some(&TargetEntity::Actor(bus::lattice::ActorIdentifier::Alias(
                "unknown/alias".into(),
            ))),
            // TODO: This should include the package name, i.e. `test-actors:foobar/foobar.foobar`
            "foobar.foobar",
            &rmp_serde::to_vec("foo").expect("failed to encode `foo`"),
        )
        .expect_err("invoked `test-actors:foobar/foobar.foobar` on unknown actor");

        let res = bus::host::call_sync(
            Some(&TargetEntity::Actor(bus::lattice::ActorIdentifier::Alias(
                "foobar-component-command-preview2".into(),
            ))),
            // TODO: This should include the package name, i.e. `test-actors:foobar/foobar.foobar`
            "foobar.foobar",
            &rmp_serde::to_vec("foo").expect("failed to encode `foo`"),
        )
        .expect("failed to invoke `test-actors:foobar/foobar.foobar` on an actor");
        let res: String = rmp_serde::from_slice(&res).expect("failed to decode response");
        assert_eq!(res, "foobar");

        let httpclient_target = TargetEntity::Link(Some("httpclient".into()));
        let buf = rmp_serde::to_vec_named(&http::ClientRequest {
            method: "PUT".into(),
            url: format!("http://localhost:{port}/test"),
            headers: HashMap::default(),
            body: b"test".to_vec(),
        })
        .expect("failed to encode `ClientRequest`");
        let buf = bus::host::call_sync(
            Some(&httpclient_target),
            "wasmcloud:httpclient/HttpClient.Request",
            &buf,
        )
        .expect("failed to perform `HttpClient.Request`");
        let http::Response {
            status_code,
            header: _, // TODO: Verify headers
            body: response_body,
        } = rmp_serde::from_slice(&buf).expect("failed to decode `HttpClient.Request` response");
        assert_eq!(status_code, 200);
        assert_eq!(response_body, b"test");

        Ok(HttpResponse {
            body: body.into(),
            ..Default::default()
        })
    }
}

export_actor!(HttpLogRng, HttpHandler);

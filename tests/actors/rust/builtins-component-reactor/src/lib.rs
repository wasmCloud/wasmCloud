wit_bindgen::generate!({
    exports: {
        world: Actor,
        "wasi:http/incoming-handler": Actor,
    }
});

use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde::Deserialize;
use serde_json::json;
use wasi::http::types;
use wasmcloud_actor::wasi::logging::logging;
use wasmcloud_actor::wasi::random::random;
use wasmcloud_actor::wasi::{blobstore, keyvalue};
use wasmcloud_actor::wasmcloud::bus::lattice::TargetEntity;
use wasmcloud_actor::wasmcloud::{bus, messaging};
use wasmcloud_actor::{
    debug, error, info, trace, warn, HostRng, InputStreamReader, OutputStreamWriter,
};

struct Actor;

impl exports::wasi::http::incoming_handler::Guest for Actor {
    fn handle(request: types::IncomingRequest, response_out: types::ResponseOutparam) {
        let _now = wasi::clocks::timezone::display(wasi::clocks::wall_clock::now());
        let _now = wasi::clocks::monotonic_clock::now();

        assert!(matches!(
            types::incoming_request_method(request),
            types::Method::Post
        ));
        assert_eq!(
            types::incoming_request_path_with_query(request).as_deref(),
            Some("/foo?bar=baz")
        );
        assert!(types::incoming_request_scheme(request).is_none());
        // NOTE: Authority is lost in translation to Smithy HttpRequest
        assert_eq!(types::incoming_request_authority(request), None);
        let headers = types::incoming_request_headers(request);

        let header_entries = types::fields_entries(headers)
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut header_iter = header_entries.clone().into_iter();

        assert_eq!(header_iter.next(), Some(("accept".into(), b"*/*".to_vec())));
        assert_eq!(types::fields_get(headers, "accept"), vec![b"*/*"]);

        assert_eq!(
            header_iter.next(),
            Some(("content-length".into(), b"21".to_vec()))
        );
        assert_eq!(types::fields_get(headers, "content-length"), vec![b"21"]);

        let (host_key, host_value) = header_iter.next().expect("`host` header missing");
        assert_eq!(host_key, "host");
        assert_eq!(types::fields_get(headers, "host"), vec![host_value]);

        assert_eq!(
            header_iter.next(),
            Some(("test-header".into(), b"test-value".to_vec()))
        );
        assert_eq!(
            types::fields_get(headers, "test-header"),
            vec![b"test-value"]
        );

        assert!(header_iter.next().is_none());

        let headers_clone = types::fields_clone(headers);
        assert_ne!(headers, headers_clone);
        types::fields_set(headers_clone, "foo", &[b"bar".to_vec()]);
        types::fields_append(headers_clone, "foo", b"baz");
        assert_eq!(
            types::fields_get(headers_clone, "foo"),
            vec![b"bar", b"baz"]
        );
        types::fields_delete(headers_clone, "foo");
        assert_eq!(
            types::fields_entries(headers_clone)
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            header_entries,
        );

        let request_stream = types::incoming_request_consume(request)
            .expect("failed to get incoming request stream");
        let mut request_body = vec![];
        InputStreamReader::from(request_stream)
            .read_to_end(&mut request_body)
            .expect("failed to read value from incoming request stream");

        #[derive(Deserialize)]
        struct Request {
            min: u32,
            max: u32,
        }
        let Request { min, max } =
            serde_json::from_slice(&request_body).expect("failed to decode request body");
        types::finish_incoming_stream(request_stream);

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
            "long_value": "1234567890".repeat(1000),
        });
        eprintln!("response: `{res:?}`");
        let body = serde_json::to_vec(&res).expect("failed to encode response to JSON");
        let response = types::new_outgoing_response(200, types::new_fields(&[]))
            .expect("failed to construct outgoing response");
        let response_stream = types::outgoing_response_write(response)
            .expect("failed to get outgoing response stream");
        let mut response_stream_writer = OutputStreamWriter::from(response_stream);
        response_stream_writer
            .write_all(&body)
            .expect("failed to write body to outgoing response stream");
        response_stream_writer
            .flush()
            .expect("failed to flush outgoing response stream");
        types::finish_outgoing_stream(response_stream);
        types::set_response_outparam(response_out, Ok(response)).expect("failed to set response");

        bus::lattice::set_target(
            Some(&TargetEntity::Link(Some("messaging".into()))),
            &[bus::lattice::target_wasmcloud_messaging_consumer()],
        );
        messaging::consumer::publish(&messaging::types::BrokerMessage {
            body: Some(body.clone()),
            reply_to: Some("noreply".into()),
            subject: "test-messaging-publish".into(),
        })
        .expect("failed to publish response");

        let messaging::types::BrokerMessage {
            body: response_body,
            reply_to,
            subject: _,
        } = messaging::consumer::request("test-messaging-request", Some(b"foo"), 1000)
            .expect("failed to request");
        assert_eq!(response_body.as_deref(), Some(b"bar".as_slice()));
        assert_eq!(reply_to, None);

        let responses = messaging::consumer::request_multi(
            "test-messaging-request-multi",
            Some(b"foo"),
            1000,
            1,
        )
        .expect("failed to request multi");
        let mut responses = responses.into_iter();
        match (responses.next(), responses.next()) {
            (
                Some(messaging::types::BrokerMessage {
                    body: response_body,
                    reply_to,
                    subject: _,
                }),
                None,
            ) => {
                assert_eq!(response_body.as_deref(), Some(b"bar".as_slice()));
                assert_eq!(reply_to, None);
            }
            (None, None) => panic!("no responses received"),
            _ => panic!("too many responses received"),
        }

        bus::lattice::set_target(
            Some(&TargetEntity::Link(Some("keyvalue".into()))),
            &[bus::lattice::target_wasi_keyvalue_readwrite()],
        );
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
        keyvalue::types::outgoing_value_write_body_sync(result_value, &body)
            .expect("failed to write outgoing value");
        keyvalue::readwrite::set(bucket, &result_key, result_value)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to set `result`");

        let result_value = keyvalue::readwrite::get(bucket, &result_key)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to get `result`");
        let result_value = keyvalue::types::incoming_value_consume_sync(result_value)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to get incoming value buffer");
        assert_eq!(result_value, body);

        let result_value = keyvalue::types::new_outgoing_value();
        let result_stream = keyvalue::types::outgoing_value_write_body_async(result_value)
            .expect("failed to get outgoing value output stream");
        let mut result_stream_writer = OutputStreamWriter::from(result_stream);
        result_stream_writer
            .write_all(&body)
            .expect("failed to write result to keyvalue output stream");
        result_stream_writer
            .flush()
            .expect("failed to flush keyvalue output stream");
        keyvalue::readwrite::set(bucket, &result_key, result_value)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to set `result`");

        bus::lattice::set_target(
            Some(&TargetEntity::Link(Some("keyvalue".into()))),
            &[bus::lattice::target_wasi_keyvalue_atomic()],
        );
        let counter_key = String::from("counter");
        let value = keyvalue::atomic::increment(bucket, &counter_key, 1)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to increment `counter`");
        assert_eq!(value, 1);
        let value = keyvalue::atomic::increment(bucket, &counter_key, 41)
            .map_err(keyvalue::wasi_cloud_error::trace)
            .expect("failed to increment `counter`");
        assert_eq!(value, 42);

        // TODO: Verify return value when implemented for all hosts
        let _ = keyvalue::atomic::compare_and_swap(bucket, &counter_key, 42, 4242);
        let _ = keyvalue::atomic::compare_and_swap(bucket, &counter_key, 4242, 42);

        bus::lattice::set_target(
            Some(&TargetEntity::Link(Some("blobstore".into()))),
            &[bus::lattice::target_wasi_blobstore_blobstore()],
        );

        let container_name = String::from("container");
        assert!(!blobstore::blobstore::container_exists(&container_name)
            .expect("failed to check whether container exists"));
        let created_container = blobstore::blobstore::create_container(&container_name)
            .expect("failed to create container");
        assert!(blobstore::blobstore::container_exists(&container_name)
            .expect("failed to check whether container exists"));
        let got_container =
            blobstore::blobstore::get_container(&container_name).expect("failed to get container");

        let blobstore::container::ContainerMetadata { name, created_at } =
            blobstore::container::info(created_container)
                .expect("failed to get info of created container");
        assert_eq!(name, "container");
        assert!(created_at > 0);

        let got_info =
            blobstore::container::info(got_container).expect("failed to get info of got container");
        assert_eq!(got_info.name, "container");
        assert_eq!(got_info.created_at, created_at);
        // NOTE: At this point we should be able to assume that created container and got container are
        // indeed the same container
        blobstore::container::drop_container(got_container);

        assert_eq!(
            blobstore::container::name(created_container).expect("failed to get container name"),
            "container"
        );

        assert!(
            !blobstore::container::has_object(created_container, &result_key)
                .expect("failed to check whether `result` object exists")
        );
        // TODO: Assert that this succeeds once providers are compatible
        let _ = blobstore::container::delete_object(created_container, &result_key);

        let result_value = blobstore::types::new_outgoing_value();
        let result_stream = blobstore::types::outgoing_value_write_body(result_value)
            .expect("failed to get outgoing value output stream");
        let mut result_stream_writer = OutputStreamWriter::from(result_stream);
        result_stream_writer
            .write_all(&body)
            .expect("failed to write result to blobstore output stream");
        result_stream_writer
            .flush()
            .expect("failed to flush blobstore output stream");
        blobstore::container::write_data(created_container, &result_key, result_value)
            .expect("failed to write `result`");

        // TODO: Expand blobstore testing procedure

        bus::host::call_sync(
            Some(&TargetEntity::Actor(bus::lattice::ActorIdentifier::Alias(
                "unknown/alias".into(),
            ))),
            "test-actors:foobar/actor.foobar",
            r#"{"arg":"foo"}"#.as_bytes(),
        )
        .expect_err("invoked `test-actors:foobar/actor.foobar` on unknown actor");

        // TODO: Use a wasifill
        let res = bus::host::call_sync(
            Some(&TargetEntity::Actor(bus::lattice::ActorIdentifier::Alias(
                "foobar-component-command-preview2".into(),
            ))),
            "test-actors:foobar/actor.foobar",
            r#"{"arg":"foo"}"#.as_bytes(),
        )
        .expect("failed to invoke `test-actors:foobar/actor.foobar` on an actor");
        let res: String = serde_json::from_slice(&res).expect("failed to decode response");
        assert_eq!(res, "foobar");
    }
}

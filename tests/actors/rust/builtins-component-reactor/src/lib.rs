wit_bindgen::generate!("actor");

use std::io::{Read, Write};

use serde::Deserialize;
use serde_json::json;
use wasi::http::http_types as types;
use wasmcloud_actor::wasi::logging::logging;
use wasmcloud_actor::wasi::random::random;
use wasmcloud_actor::wasi::{blobstore, keyvalue};
use wasmcloud_actor::wasmcloud::bus::lattice::TargetEntity;
use wasmcloud_actor::wasmcloud::{bus, messaging};
use wasmcloud_actor::{
    debug, error, info, trace, warn, HostRng, InputStreamReader, OutputStreamWriter,
};

struct Actor;

impl exports::wasi::http::incoming_handler::IncomingHandler for Actor {
    fn handle(request: types::IncomingRequest, response_out: types::ResponseOutparam) {
        assert!(matches!(
            types::incoming_request_method(request),
            types::Method::Post
        ));
        assert_eq!(
            types::incoming_request_path_with_query(request).as_deref(),
            Some("/")
        );
        assert!(types::incoming_request_scheme(request).is_none());
        // NOTE: Authority is lost in traslation to Smithy HttpRequest
        assert_eq!(types::incoming_request_authority(request), None);
        let _headers = types::incoming_request_headers(request);
        // TODO: Validate headers
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
            .expect("failed to create response"); // TODO: Set headers
        let response_stream = types::outgoing_response_write(response)
            .expect("failed to get outgoing response stream");
        let n = OutputStreamWriter::from(response_stream)
            .write(&body)
            .expect("failed to write body to outgoing response stream");
        assert_eq!(n, body.len());
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
        let result_stream = keyvalue::types::outgoing_value_write_body(result_value)
            .expect("failed to get outgoing value output stream");
        let n = OutputStreamWriter::from(result_stream)
            .write(&body)
            .expect("failed to write result to keyvalue output stream");
        assert_eq!(n, body.len());
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
        let n = OutputStreamWriter::from(result_stream)
            .write(&body)
            .expect("failed to write result to blobstore output stream");
        assert_eq!(n, body.len());
        blobstore::container::write_data(created_container, &result_key, result_value)
            .expect("failed to write `result`");

        // TODO: Expand blobstore testing procedure
    }
}

export_actor!(Actor);

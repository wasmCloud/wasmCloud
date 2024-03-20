#![allow(clippy::missing_safety_doc)]

wit_bindgen::generate!({
    world: "actor",
    with: {
        "wasi:io/streams@0.2.0": wasmcloud_actor::wasi::io::streams,
    }
});

use std::io::{Read, Write};

use serde::Deserialize;
use serde_json::json;
use test_actors::testing::*;
use wasi::http;
use wasi::io::poll::poll;
use wasi::sockets::{instance_network, network, tcp_create_socket, udp_create_socket};
use wasi::{blobstore, keyvalue};
use wasmcloud_actor::wasi::logging::logging;
use wasmcloud_actor::wasi::random::random;
use wasmcloud_actor::wasmcloud::bus;
use wasmcloud_actor::{
    debug, error, info, trace, warn, HostRng, InputStreamReader, OutputStreamWriter,
};

struct Actor;

fn test_blobstore(min_created_at: u64, body: &[u8], name: impl Into<String>) {
    fn assert_create_container(
        name: &String,
        min_created_at: u64,
    ) -> blobstore::container::Container {
        eprintln!("call `wasi:blobstore/blobstore.create-container`...");
        let container =
            blobstore::blobstore::create_container(name).expect("failed to create container");
        eprintln!("call `wasi:blobstore/blobstore.container-exists`...");
        assert_eq!(
            &container.name().expect("failed to get container name"),
            name,
        );
        let md = container
            .info()
            .expect("failed to get info of created container");
        assert_eq!(&md.name, name);
        assert!(md.created_at >= min_created_at);

        assert!(blobstore::blobstore::container_exists(name)
            .expect("failed to check whether container exists"));
        eprintln!("call `wasi:blobstore/container.container.info`...");
        {
            eprintln!("call `wasi:blobstore/blobstore.get-container`...");
            let container =
                blobstore::blobstore::get_container(name).expect("failed to get container");
            eprintln!("call `wasi:blobstore/container.container.info`...");
            let info = container
                .info()
                .expect("failed to get info of got container");
            assert_eq!(info.name, md.name);
            assert_eq!(info.created_at, md.created_at);
        }
        {
            eprintln!("call `wasi:blobstore/blobstore.create-container`...");
            let container = blobstore::blobstore::create_container(name)
                .expect("failed to create existing container");
            eprintln!("call `wasi:blobstore/container.container.info`...");
            let info = container
                .info()
                .expect("failed to get info of created container");
            assert_eq!(info.name, md.name);
            assert_eq!(info.created_at, md.created_at);
        }
        container
            .clear()
            .expect("failed to clear an empty container");
        eprintln!(
            "call `wasi:blobstore/container.container.list-objects` on an empty container..."
        );
        let objects = container
            .list_objects()
            .expect("failed to list objects in an empty container");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names` on an empty container..."
        );
        let (names, end) = objects
            .read_stream_object_names(10)
            .expect("failed to read object names from an empty container");
        assert!(end);
        assert!(names.is_empty());
        container
    }

    fn assert_write_container_data(
        container: &blobstore::container::Container,
        key: &String,
        data: &[u8],
    ) {
        let value = blobstore::types::OutgoingValue::new_outgoing_value();
        let mut value_stream = value
            .outgoing_value_write_body()
            .expect("failed to get outgoing value output stream");
        let mut value_stream_writer = OutputStreamWriter::from(&mut value_stream);
        eprintln!("write body to outgoing blobstore stream...");
        value_stream_writer
            .write_all(data)
            .expect("failed to write result to blobstore output stream");
        eprintln!("flush outgoing blobstore stream...");
        value_stream_writer
            .flush()
            .expect("failed to flush blobstore output stream");
        eprintln!("call `wasi:blobstore/container.container.write-data`...");
        container
            .write_data(key, &value)
            .expect("failed to write data");
        eprintln!("call `wasi:blobstore/container.container.get-data`...");
        let stored_value = container
            .get_data(
                key,
                0,
                data.len().saturating_add(10).try_into().unwrap_or(u64::MAX),
            )
            .expect("failed to get container data");
        let stored_value =
            blobstore::types::IncomingValue::incoming_value_consume_sync(stored_value)
                .expect("failed to get stored value buffer");
        assert_eq!(stored_value, data);
    }

    let name = name.into();

    eprintln!("create `{name}` container");
    let container = assert_create_container(&name, min_created_at);

    eprintln!("call `wasi:blobstore/container.container.has-object`...");
    assert!(!container
        .has_object(&String::from("result"))
        .expect("failed to check whether `result` object exists"));
    eprintln!("call `wasi:blobstore/container.container.delete-object`...");
    container
        .delete_object(&String::from("result"))
        .expect("failed to delete object");

    eprintln!("write `foo` object...");
    assert_write_container_data(&container, &String::from("foo"), b"foo");

    eprintln!("rewrite `foo` object...");
    assert_write_container_data(&container, &String::from("foo"), b"newfoo");

    eprintln!("write `bar` object...");
    assert_write_container_data(&container, &String::from("bar"), b"bar");

    eprintln!("write `baz` object...");
    assert_write_container_data(&container, &String::from("baz"), b"baz");

    eprintln!("write `result` object...");
    assert_write_container_data(&container, &String::from("result"), body);

    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = container
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (mut first_names, end) = objects
            .read_stream_object_names(2)
            .expect("failed to read object names");
        assert!(!end);

        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (mut names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);

        names.append(&mut first_names);
        names.sort();
        assert_eq!(names, ["bar", "baz", "foo", "result"]);
    }

    let name_other = format!("{name}-other");
    eprintln!("create `{name_other}` container");
    let other = assert_create_container(&name_other, min_created_at);

    let container_foo = blobstore::types::ObjectId {
        container: name.clone(),
        object: String::from("foo"),
    };

    let other_foobar = blobstore::types::ObjectId {
        container: name_other.clone(),
        object: String::from("foobar"),
    };

    eprintln!("call `wasi:blobstore/blobstore.move-object`...");
    blobstore::blobstore::move_object(&other_foobar, &container_foo)
        .expect_err("should not be possible to move non-existing object");

    eprintln!("call `wasi:blobstore/blobstore.move-object`...");
    blobstore::blobstore::move_object(&container_foo, &other_foobar)
        .expect("failed to move object");
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = container
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (mut names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);
        names.sort();
        assert_eq!(names, ["bar", "baz", "result"]);
    }
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = other
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (names, end) = objects
            .read_stream_object_names(2)
            .expect("failed to read object names");
        assert!(end);
        assert_eq!(names, ["foobar"])
    };
    eprintln!("call `wasi:blobstore/container.container.get-data`...");
    let value = other
        .get_data(&String::from("foobar"), 3, 10)
        .expect("failed to get `foobar` object from `other` container");
    let value = blobstore::types::IncomingValue::incoming_value_consume_sync(value)
        .expect("failed to get stored value buffer");
    assert_eq!(value, b"foo");

    eprintln!("call `wasi:blobstore/blobstore.copy-object`...");
    blobstore::blobstore::copy_object(&container_foo, &other_foobar)
        .expect_err("should not be possible to copy a non-existing object");

    eprintln!("call `wasi:blobstore/blobstore.copy-object`...");
    blobstore::blobstore::copy_object(&other_foobar, &container_foo)
        .expect("failed to copy object");
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = other
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (names, end) = objects
            .read_stream_object_names(2)
            .expect("failed to read object names");
        assert!(end);
        assert_eq!(names, ["foobar"]);
    }
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = container
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (mut names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);
        names.sort();
        assert_eq!(names, ["bar", "baz", "foo", "result"]);
    }
    eprintln!("call `wasi:blobstore/container.container.get-data`...");
    let value = container
        .get_data(&String::from("foo"), 0, 9999)
        .expect("failed to get `foobar` object from `other` container");
    let value = blobstore::types::IncomingValue::incoming_value_consume_sync(value)
        .expect("failed to get stored value buffer");
    assert_eq!(value, b"newfoo");

    container.clear().expect("failed to clear container");
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = container
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);
        assert!(names.is_empty());
    }
    container
        .delete_object(&String::from("foo"))
        .expect("failed to delete non-existing object");
    eprintln!("call `wasi:blobstore/blobstore.delete-container`...");
    blobstore::blobstore::delete_container(&name).expect("failed to delete container");
    eprintln!("call `wasi:blobstore/blobstore.get-container`...");
    blobstore::blobstore::get_container(&name).expect_err("container should have been deleted");
    eprintln!("call `wasi:blobstore/blobstore.container-exists`...");
    assert!(
        !blobstore::blobstore::container_exists(&name).expect("container should have been deleted")
    );

    eprintln!("call `wasi:blobstore/blobstore.delete-object`...");
    other
        .delete_object(&String::from("foobar"))
        .expect("failed to delete object");
    {
        eprintln!("call `wasi:blobstore/container.container.list-objects`...");
        let objects = other
            .list_objects()
            .expect("failed to list container objects");
        eprintln!(
            "call `wasi:blobstore/container.stream-object-names.read-stream-object-names`..."
        );
        let (names, end) = objects
            .read_stream_object_names(5)
            .expect("failed to read object names");
        assert!(end);
        assert!(names.is_empty());
    }
    eprintln!("call `wasi:blobstore/blobstore.delete-container`...");
    blobstore::blobstore::delete_container(&name_other).expect("failed to delete container");
    eprintln!("call `wasi:blobstore/blobstore.get-container`...");
    blobstore::blobstore::get_container(&name_other)
        .expect_err("container should have been deleted");
    eprintln!("call `wasi:blobstore/blobstore.container-exists`...");
    assert!(!blobstore::blobstore::container_exists(&name_other)
        .expect("container should have been deleted"));
}

impl exports::wasi::http::incoming_handler::Guest for Actor {
    fn handle(request: http::types::IncomingRequest, response_out: http::types::ResponseOutparam) {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Request {
            authority: String,
            min: u32,
            max: u32,
            config_key: String,
        }

        let path_with_query = request.path_with_query();
        if path_with_query.as_deref() == Some("/echo") {
            assert!(matches!(request.method(), http::types::Method::Put));

            let response = http::types::OutgoingResponse::new(http::types::Fields::new());
            let response_body = response
                .body()
                .expect("failed to get outgoing response body");
            let request_body = request
                .consume()
                .expect("failed to get incoming request body");
            http::types::ResponseOutparam::set(response_out, Ok(response));
            {
                let input_stream = request_body
                    .stream()
                    .expect("failed to get incoming request stream");
                let output_stream = response_body
                    .write()
                    .expect("failed to get outgoing response stream");

                eprintln!("[echo] read `t`...");
                assert_eq!(
                    input_stream.blocking_read(1).expect("failed to read `t`"),
                    b"t"
                );

                eprintln!("[echo] write `t`...");
                output_stream
                    .blocking_write_and_flush(b"t")
                    .expect("failed to write `t`");

                eprintln!("[echo] splice `es`...");
                let n = output_stream
                    .blocking_splice(&input_stream, 2)
                    .expect("failed to splice");
                assert_eq!(n, 2);

                eprintln!("[echo] read `t`...");
                assert_eq!(
                    input_stream.blocking_read(1).expect("failed to read `t`"),
                    b"t"
                );

                eprintln!("[echo] write `t`...");
                output_stream
                    .blocking_write_and_flush(b"t")
                    .expect("failed to write `t`");

                eprintln!("[echo] read `i`...");
                assert_eq!(
                    input_stream.blocking_read(1).expect("failed to read `i`"),
                    b"i"
                );
                eprintln!("[echo] write `i`...");
                output_stream
                    .blocking_write_and_flush(b"i")
                    .expect("failed to write `i`");

                eprintln!("[echo] read `n`...");
                assert_eq!(
                    input_stream.blocking_read(1).expect("failed to read `n`"),
                    b"n"
                );
                eprintln!("[echo] read `g`...");
                assert_eq!(
                    input_stream.blocking_read(1).expect("failed to read `g`"),
                    b"g"
                );
                eprintln!("[echo] write `ng`...");
                output_stream
                    .blocking_write_and_flush(b"ng")
                    .expect("failed to write `ng`");
                assert!(input_stream.blocking_read(1).is_err());
            };
            let _trailers = http::types::IncomingBody::finish(request_body);
            http::types::OutgoingBody::finish(response_body, None)
                .expect("failed to finish response body");
            return;
        }

        assert!(matches!(request.method(), http::types::Method::Post));
        assert_eq!(request.path_with_query().as_deref(), Some("/foo?bar=baz"));
        assert!(matches!(request.scheme(), Some(http::types::Scheme::Http)));
        // NOTE: This will be validated after the body has been received
        let request_authority = request.authority().expect("authority missing");
        let headers = request.headers();

        let mut header_entries = headers.entries();
        header_entries.sort();
        let mut header_iter = header_entries.clone().into_iter();

        assert_eq!(header_iter.next(), Some(("accept".into(), b"*/*".to_vec())));
        assert_eq!(headers.get(&String::from("accept")), vec![b"*/*"]);

        let (content_length_key, content_length_value) =
            header_iter.next().expect("`content-length` header missing");
        assert_eq!(content_length_key, "content-length");
        assert_eq!(
            headers.get(&String::from("content-length")),
            vec![content_length_value.as_slice()]
        );
        let content_length: usize = String::from_utf8(content_length_value)
            .expect("`content-length` value is not a valid string")
            .parse()
            .expect("`content-length` value is not a valid usize");

        let (host_key, host_value) = header_iter.next().expect("`host` header missing");
        assert_eq!(host_key, "host");
        assert_eq!(
            headers.get(&String::from("host")),
            vec![host_value.as_slice()]
        );
        let host_value =
            String::from_utf8(host_value).expect("`host` header is not a valid string");

        assert_eq!(
            header_iter.next(),
            Some(("test-header".into(), b"test-value".to_vec()))
        );
        assert_eq!(
            headers.get(&String::from("test-header")),
            vec![b"test-value"]
        );
        assert!(header_iter.next().is_none());

        let headers_clone = headers.clone();
        headers_clone
            .set(&String::from("foo"), &[b"bar".to_vec()])
            .expect("failed to set `foo` header");
        headers_clone
            .append(&String::from("foo"), &b"baz".to_vec())
            .expect("failed to append `foo` header");
        assert_eq!(
            headers_clone.get(&String::from("foo")),
            vec![b"bar", b"baz"]
        );
        headers_clone
            .delete(&String::from("foo"))
            .expect("failed to delete `foo` header");
        let mut headers_clone = headers_clone.entries();
        headers_clone.sort();
        assert_eq!(headers_clone, header_entries);

        let request_body = request
            .consume()
            .expect("failed to get incoming request body");
        let Request {
            authority,
            min,
            max,
            config_key,
        } = {
            let mut buf = vec![];
            let mut stream = request_body
                .stream()
                .expect("failed to get incoming request stream");
            InputStreamReader::from(&mut stream)
                .read_to_end(&mut buf)
                .expect("failed to read value from incoming request stream");
            assert_eq!(buf.len(), content_length);
            serde_json::from_slice(&buf).expect("failed to decode request body")
        };
        let _trailers = http::types::IncomingBody::finish(request_body);

        assert_eq!(host_value, authority);
        assert_eq!(request_authority, authority);

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

        // No args, return string
        let pong = pingpong::ping();
        // Number arg, return number
        let meaning_of_universe = busybox::increment_number(41);
        // Multiple args, return vector of strings
        let other: Vec<String> = busybox::string_split("hi,there,friend", ',');
        // Variant / Enum argument, return bool
        let is_same = busybox::string_assert(busybox::Easyasonetwothree::A, "a");
        let doggo = busybox::Dog {
            name: "Archie".to_string(),
            age: 3,
        };
        // Record / struct argument
        let is_good_boy = busybox::is_good_boy(&doggo);

        let res = json!({
            "get_random_bytes": random::get_random_bytes(8),
            "get_random_u64": random::get_random_u64(),
            "guid": HostRng::generate_guid(),
            "random_32": HostRng::random32(),
            "random_in_range": HostRng::random_in_range(min, max),
            "long_value": "1234567890".repeat(5000),
            "config_value": wasmcloud::bus::guest_config::get(&config_key).expect("failed to get config value"),
            "all_config": wasmcloud::bus::guest_config::get_all().expect("failed to get all config values"),
            "ping": pong,
            "meaning_of_universe": meaning_of_universe,
            "split": other,
            "is_same": is_same,
            "archie": is_good_boy,
        });
        eprintln!("response: `{res:?}`");

        let body = serde_json::to_vec(&res).expect("failed to encode response to JSON");

        let outgoing_request = http::types::OutgoingRequest::new(http::types::Fields::new());
        outgoing_request
            .set_method(&http::types::Method::Put)
            .expect("failed to set request method");
        outgoing_request
            .set_path_with_query(Some("/echo"))
            .expect("failed to set request path with query");
        outgoing_request
            .set_scheme(Some(&http::types::Scheme::Http))
            .expect("failed to set request scheme");
        outgoing_request
            .set_authority(Some(&authority))
            .expect("failed to set request authority");
        let outgoing_request_body = outgoing_request
            .body()
            .expect("failed to get outgoing request body");
        let outgoing_request_response = http::outgoing_handler::handle(outgoing_request, None)
            .expect("failed to handle HTTP request");
        let outgoing_request_response_sub = outgoing_request_response.subscribe();

        let tcp4 = tcp_create_socket::create_tcp_socket(network::IpAddressFamily::Ipv4)
            .expect("failed to create an IPv4 TCP socket");
        let tcp6 = tcp_create_socket::create_tcp_socket(network::IpAddressFamily::Ipv6)
            .expect("failed to create an IPv6 TCP socket");
        let udp4 = udp_create_socket::create_udp_socket(network::IpAddressFamily::Ipv4)
            .expect("failed to create an IPv4 UDP socket");
        let udp6 = udp_create_socket::create_udp_socket(network::IpAddressFamily::Ipv6)
            .expect("failed to create an IPv6 UDP socket");
        tcp4.start_bind(
            &instance_network::instance_network(),
            network::IpSocketAddress::Ipv4(network::Ipv4SocketAddress {
                port: 0,
                address: (0, 0, 0, 0),
            }),
        )
        .expect_err("should not be able to bind to any IPv4 address on TCP");
        tcp6.start_bind(
            &instance_network::instance_network(),
            network::IpSocketAddress::Ipv6(network::Ipv6SocketAddress {
                port: 0,
                address: (0, 0, 0, 0, 0, 0, 0, 0),
                flow_info: 0,
                scope_id: 0,
            }),
        )
        .expect_err("should not be able to bind to any IPv6 address on TCP");
        udp4.start_bind(
            &instance_network::instance_network(),
            network::IpSocketAddress::Ipv4(network::Ipv4SocketAddress {
                port: 0,
                address: (0, 0, 0, 0),
            }),
        )
        .expect_err("should not be able to bind to any IPv4 address on UDP");
        udp6.start_bind(
            &instance_network::instance_network(),
            network::IpSocketAddress::Ipv6(network::Ipv6SocketAddress {
                port: 0,
                address: (0, 0, 0, 0, 0, 0, 0, 0),
                flow_info: 0,
                scope_id: 0,
            }),
        )
        .expect_err("should not be able to bind to any IPv6 address on UDP");

        eprintln!("poll outgoing HTTP request response...");
        assert_eq!(poll(&[&outgoing_request_response_sub]), [0]);
        let outgoing_request_response = outgoing_request_response
            .get()
            .expect("HTTP request response missing")
            .expect("HTTP request response requested more than once")
            .expect("HTTP request failed");
        assert_eq!(outgoing_request_response.status(), 200);

        // TODO: Assert headers
        _ = outgoing_request_response.headers();
        let outgoing_request_response_body = outgoing_request_response
            .consume()
            .expect("failed to get incoming request body");
        {
            let input_stream = outgoing_request_response_body
                .stream()
                .expect("failed to get HTTP request response stream");
            let output_stream = outgoing_request_body
                .write()
                .expect("failed to get outgoing request stream");

            eprintln!("write `t`...");
            output_stream
                .blocking_write_and_flush(b"t")
                .expect("failed to write `t`");
            eprintln!("read `t`...");
            assert_eq!(
                input_stream.blocking_read(1).expect("failed to read `t`"),
                b"t"
            );

            eprintln!("write `est`...");
            output_stream
                .blocking_write_and_flush(b"est")
                .expect("failed to write `est`");
            eprintln!("read `e`...");
            assert_eq!(
                input_stream.blocking_read(1).expect("failed to read `e`"),
                b"e"
            );
            eprintln!("read `s`...");
            assert_eq!(
                input_stream.blocking_read(1).expect("failed to read `s`"),
                b"s"
            );
            eprintln!("read `t`...");
            assert_eq!(
                input_stream.blocking_read(1).expect("failed to read `t`"),
                b"t"
            );

            eprintln!("write `i`...");
            output_stream
                .blocking_write_and_flush(b"i")
                .expect("failed to write `i`");
            eprintln!("read `i`...");
            assert_eq!(
                input_stream.blocking_read(1).expect("failed to read `i`"),
                b"i"
            );

            eprintln!("write `ng`...");
            output_stream
                .blocking_write_and_flush(b"ng")
                .expect("failed to write `ng`");
            eprintln!("read `n`...");
            assert_eq!(
                input_stream.blocking_read(1).expect("failed to read `n`"),
                b"n"
            );
            eprintln!("read `g`...");
            assert_eq!(
                input_stream.blocking_read(1).expect("failed to read `g`"),
                b"g"
            );
        }
        eprintln!("set response");
        http::types::OutgoingBody::finish(outgoing_request_body, None)
            .expect("failed to finish sending request body");
        // TODO: Assert trailers
        let _trailers = http::types::IncomingBody::finish(outgoing_request_response_body);

        let response = http::types::OutgoingResponse::new(http::types::Fields::new());
        let response_body = response
            .body()
            .expect("failed to get outgoing response body");
        {
            let mut stream = response_body
                .write()
                .expect("failed to get outgoing response stream");
            let mut w = OutputStreamWriter::from(&mut stream);
            w.write_all(&body)
                .expect("failed to write body to outgoing response stream");
            w.flush().expect("failed to flush outgoing response stream");
        }
        http::types::OutgoingBody::finish(response_body, None)
            .expect("failed to finish response body");

        let foo_key = String::from("foo");
        let bucket = keyvalue::types::Bucket::open_bucket("")
            .map_err(|e| e.trace())
            .expect("failed to open empty bucket");
        eprintln!("call `wasi:keyvalue/eventual.exists`...");
        keyvalue::eventual::exists(&bucket, &foo_key)
            .map_err(|e| e.trace())
            .expect("failed to check whether `foo` exists")
            .then_some(())
            .expect("`foo` does not exist");

        eprintln!("call `wasi:keyvalue/eventual.get`...");
        let foo_value = keyvalue::eventual::get(&bucket, &foo_key)
            .map_err(|e| e.trace())
            .expect("failed to get `foo`")
            .expect("`foo` does not exist in bucket");
        assert!(foo_value.incoming_value_size().is_err());

        let foo_value = keyvalue::types::IncomingValue::incoming_value_consume_sync(foo_value)
            .map_err(|e| e.trace())
            .expect("failed to get incoming value buffer");
        assert_eq!(foo_value, b"bar");

        eprintln!("call `wasi:keyvalue/eventual.get`...");
        let foo_value = keyvalue::eventual::get(&bucket, &foo_key)
            .map_err(|e| e.trace())
            .expect("failed to get `foo`")
            .expect("`foo` does not exist in bucket");
        let mut foo_stream =
            keyvalue::types::IncomingValue::incoming_value_consume_async(foo_value)
                .map_err(|e| e.trace())
                .expect("failed to get incoming value stream");
        let mut foo_value = vec![];
        let n = InputStreamReader::from(&mut foo_stream)
            .read_to_end(&mut foo_value)
            .expect("failed to read value from keyvalue input stream");
        assert_eq!(n, 3);
        assert_eq!(foo_value, b"bar");

        eprintln!("call `wasi:keyvalue/eventual.delete`...");
        keyvalue::eventual::delete(&bucket, &foo_key)
            .map_err(|e| e.trace())
            .expect("failed to delete `foo`");

        eprintln!("call `wasi:keyvalue/eventual.exists`...");
        let foo_exists = keyvalue::eventual::exists(&bucket, &foo_key)
            .map_err(|e| e.trace())
            .expect(
                "`exists` method should not have returned an error for `foo` key, which was deleted",
            );
        assert!(!foo_exists);

        let result_key = String::from("result");

        let result_value = keyvalue::types::OutgoingValue::new_outgoing_value();
        result_value
            .outgoing_value_write_body_sync(&body)
            .expect("failed to write outgoing value");
        keyvalue::eventual::set(&bucket, &result_key, &result_value)
            .map_err(|e| e.trace())
            .expect("failed to set `result`");

        eprintln!("call `wasi:keyvalue/eventual.get`...");
        let result_value = keyvalue::eventual::get(&bucket, &result_key)
            .map_err(|e| e.trace())
            .expect("failed to get `result`")
            .expect("`result` does not exist in bucket");
        let result_value =
            keyvalue::types::IncomingValue::incoming_value_consume_sync(result_value)
                .map_err(|e| e.trace())
                .expect("failed to get incoming value buffer");
        assert_eq!(result_value, body);

        let result_value = keyvalue::types::OutgoingValue::new_outgoing_value();
        let mut result_stream = result_value
            .outgoing_value_write_body_async()
            .expect("failed to get outgoing value output stream");
        let mut result_stream_writer = OutputStreamWriter::from(&mut result_stream);
        result_stream_writer
            .write_all(&body)
            .expect("failed to write result to keyvalue output stream");
        result_stream_writer
            .flush()
            .expect("failed to flush keyvalue output stream");

        eprintln!("call `wasi:keyvalue/eventual.set`...");
        keyvalue::eventual::set(&bucket, &result_key, &result_value)
            .map_err(|e| e.trace())
            .expect("failed to set `result`");

        let counter_key = String::from("counter");
        eprintln!("call `wasi:keyvalue/atomic.increment`...");
        let value = keyvalue::atomic::increment(&bucket, &counter_key, 1)
            .map_err(|e| e.trace())
            .expect("failed to increment `counter`");
        assert_eq!(value, 1);
        eprintln!("call `wasi:keyvalue/atomic.increment`...");
        let value = keyvalue::atomic::increment(&bucket, &counter_key, 41)
            .map_err(|e| e.trace())
            .expect("failed to increment `counter`");
        assert_eq!(value, 42);

        eprintln!("call `wasi:keyvalue/atomic.compare-and-swap`...");
        assert!(
            keyvalue::atomic::compare_and_swap(&bucket, &counter_key, 42, 4242)
                .expect("failed to compare and swap")
        );
        eprintln!("call `wasi:keyvalue/atomic.compare-and-swap`...");
        assert!(
            !keyvalue::atomic::compare_and_swap(&bucket, &counter_key, 42, 4242)
                .expect("failed to compare and swap")
        );
        eprintln!("call `wasi:keyvalue/atomic.compare-and-swap`...");
        assert!(
            keyvalue::atomic::compare_and_swap(&bucket, &counter_key, 4242, 42)
                .expect("failed to compare and swap")
        );

        eprintln!("test default blobstore...");
        test_blobstore(1, &body, "container");

        eprintln!("test s3 blobstore...");
        bus::lattice::set_link_name(
            "s3",
            vec![bus::lattice::CallTargetInterface::new(
                "wasi",
                "blobstore",
                "blobstore",
                None,
            )],
        );
        test_blobstore(0, &body, "container");

        http::types::ResponseOutparam::set(response_out, Ok(response));
    }
}

export!(Actor);

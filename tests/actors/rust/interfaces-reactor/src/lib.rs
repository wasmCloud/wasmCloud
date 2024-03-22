#![allow(clippy::missing_safety_doc)]

wit_bindgen::generate!({
    world: "actor",
    with: {
        "wasi:http/types@0.2.0": wasmcloud_actor::wasi::http::types,
        "wasi:io/streams@0.2.0": wasmcloud_actor::wasi::io::streams,
        "wasmcloud:messaging/types": wasmcloud_actor::wasmcloud::messaging::types,
    }
});

mod blobstore;
mod http;
mod keyvalue;
mod messaging;

use serde::Deserialize;
use serde_json::json;
use test_actors::testing::*;
use wasi::sockets::{instance_network, network, tcp_create_socket, udp_create_socket};
use wasmcloud_actor::wasi::logging::logging;
use wasmcloud_actor::wasi::random::random;
use wasmcloud_actor::wasmcloud::bus;
use wasmcloud_actor::{debug, error, info, trace, warn, HostRng};

pub struct Actor;

pub fn run_test(body: &[u8]) -> (Vec<u8>, String) {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        authority: String,
        min: u32,
        max: u32,
        config_key: String,
    }
    let Request {
        authority,
        min,
        max,
        config_key,
    } = serde_json::from_slice(body).expect("failed to decode request body");

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
        "config_value": bus::guest_config::get(&config_key).expect("failed to get config value"),
        "all_config": bus::guest_config::get_all().expect("failed to get all config values"),
        "ping": pong,
        "meaning_of_universe": meaning_of_universe,
        "split": other,
        "is_same": is_same,
        "archie": is_good_boy,
    });
    eprintln!("response: `{res:?}`");

    let body = serde_json::to_vec(&res).expect("failed to encode response to JSON");

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

    eprintln!("test default messaging...");
    messaging::run_test();

    eprintln!("test default keyvalue/eventual...");
    keyvalue::run_eventual_test(&body);

    eprintln!("test vault keyvalue/eventual...");
    bus::lattice::set_link_name(
        "vault",
        vec![bus::lattice::CallTargetInterface::new(
            "wasi", "keyvalue", "eventual",
        )],
    );
    keyvalue::run_eventual_test(&body);

    eprintln!("test default keyvalue/atomic...");
    keyvalue::run_atomic_test();

    eprintln!("test default blobstore...");
    blobstore::run_test(1, &body, "container");

    eprintln!("test s3 blobstore...");
    bus::lattice::set_link_name(
        "s3",
        vec![bus::lattice::CallTargetInterface::new(
            "wasi",
            "blobstore",
            "blobstore",
        )],
    );
    blobstore::run_test(0, &body, "container");

    (body, authority)
}

export!(Actor);

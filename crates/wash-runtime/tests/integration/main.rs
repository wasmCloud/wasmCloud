//! wash-runtime's integration tests, as modules of one test binary.
//!
//! Every `tests/*.rs` file is a binary of its own that links all of
//! wash-runtime, wasmtime and cranelift, so a suite of separate files spends
//! most of its build linking. A module here costs only its own compile.
//!
//! The modules share a process: a module installs its tracing subscriber
//! with `try_init`, leaves environment variables alone, and holds
//! [`common::STDERR`] while it redirects stderr. A test that needs a
//! process to itself stays a `tests/*.rs` file and says why.

#![cfg_attr(coverage_nightly, recursion_limit = "256")]

#[path = "../common/mod.rs"]
mod common;

mod blobstore_async_multiplexed;
mod blobstore_default_p3;
mod blobstore_implements_p3;
mod blobstore_s3;
mod command;
mod component_host_plugin;
mod component_plugin_implements;
mod component_plugin_lifecycle;
mod component_plugin_secret_source;
mod cron_service;
mod cross_store_bridge;
mod density_pool;
mod fuel_metering;
mod guest_implements_routing;
mod guest_memory_budget;
mod host_plugin_loader;
mod http_allowed_hosts;
mod http_blobstore;
mod http_counter;
mod http_egress_pooling;
mod http_ip_name_lookup;
mod http_local_routing;
mod http_tls;
mod http_webgpu;
mod ingress_release;
mod instance_concurrency;
mod inter_component_call;
mod keyvalue_aba_implements_p3;
mod keyvalue_async_coexistence;
mod keyvalue_coexistence;
mod keyvalue_default_p3;
mod keyvalue_egress;
mod keyvalue_implements;
mod keyvalue_multiplexed;
mod keyvalue_redis_interop;
mod messaging_async;
mod messaging_concurrency;
mod messaging_multiplexed;
mod multi_component_config;
mod nats_blobstore;
mod nats_messaging;
mod nats_messaging_fault;
mod nats_plugin;
mod oci_cache;
mod p2_borrow_passing;
mod p2_p3_matrix;
mod p2_udp_progress;
mod p3_borrow_passing;
mod p3_ephemeral;
mod p3_loopback_service;
mod p3_resources;
mod p3_sockets;
mod p3_streams;
mod plugin_calls_workload;
mod plugin_dispatch;
mod plugin_http_egress;
mod plugin_imports_native;
mod postgres_async;
mod postgres_implements;
mod postgres_multiplexed;
mod postgres_streaming;
mod router_dev;
mod router_dynamic;
mod scale_density;
mod scratch_volumes;
mod secrets_plugin;
mod service_http;
mod service_no_cli_run;
mod socket_name_grant;
mod start_off_runtime;
mod structured_payloads;
mod thundering_herd;
mod tls_socket;
mod trigger_service_messaging;
mod warm_instances;
mod washlet_api;
mod wasi_override_versions;
mod wasi_overrides;
mod wasip3;

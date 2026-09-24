//! Generated host bindings for `wasmcloud:tls@0.1.0` and the upstream
//! `wasi:tls@0.3.0-draft` it mirrors. Both packages map their resources onto
//! the same Rust types, so one implementation serves either.

crate::wasmtime::component::bindgen!({
    world: "plugin-tls",
    imports: {
        "wasmcloud:tls/dialer.[method]connection.send": store | trappable | tracing,
        "wasmcloud:tls/dialer.[method]connection.receive": store | trappable | tracing,
        "wasmcloud:tls/client.[method]connector.send": store | trappable | tracing,
        "wasmcloud:tls/client.[method]connector.receive": store | trappable | tracing,
        "wasi:tls/client.[method]connector.send": store | trappable | tracing,
        "wasi:tls/client.[method]connector.receive": store | trappable | tracing,
        default: trappable | tracing,
    },
    with: {
        "wasmcloud:tls/dialer.connection": super::Connection,
        "wasmcloud:tls/client.connector": super::Connector,
        "wasmcloud:tls/types.error": super::TlsError,
        "wasi:tls/client.connector": super::Connector,
        "wasi:tls/types.error": super::TlsError,
    },
    require_store_data_send: true,
});

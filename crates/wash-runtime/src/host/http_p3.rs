//! P3 HTTP handler for WASIP3 components.
//!
//! This module provides the HTTP request handling path for components that
//! target WASIP3's `wasi:http/handler` interface. It uses wasmtime-wasi-http's
//! P3 `ServicePre`/`Service` to invoke the component.

use crate::engine::ctx::SharedCtx;
use crate::observability::FuelConsumptionMeter;
use http_body_util::BodyExt;
use tracing::error;
use wasmtime::Store;
use wasmtime::component::InstancePre;
use wasmtime_wasi_http::p3::bindings::ServicePre;
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

type P3Body = http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, ErrorCode>;

/// Handle an HTTP request using the WASIP3 `wasi:http/handler` interface.
///
/// P3 uses `ServicePre`/`Service` with `Store::run_concurrent` to get an
/// `Accessor` for concurrent component-model async operations.
pub async fn handle_component_request_p3(
    mut store: Store<SharedCtx>,
    pre: InstancePre<SharedCtx>,
    req: hyper::Request<hyper::body::Incoming>,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<hyper::Response<P3Body>> {
    let _ = &fuel_meter; // fuel metering integration deferred to match P2's observe() pattern

    let service_pre = ServicePre::new(pre)
        .map_err(|e| anyhow::anyhow!(e).context("failed to create P3 ServicePre"))?;

    // Convert the hyper request body — map error type since hyper::Error doesn't impl Into<ErrorCode>
    let (parts, body) = req.into_parts();
    let body = body
        .map_err(|e| ErrorCode::InternalError(Some(e.to_string())))
        .boxed_unsync();
    let req = hyper::Request::from_parts(parts, body);
    let (wasi_req, req_io) = wasmtime_wasi_http::p3::Request::from_http(req);

    // Instantiate the service
    let service = service_pre
        .instantiate_async(&mut store)
        .await
        .map_err(|e| anyhow::anyhow!(e).context("failed to instantiate P3 service"))?;

    // Use run_concurrent to get an Accessor for the P3 async component model.
    // run_concurrent returns Result<R> where R is the closure return.
    let result: Result<wasmtime_wasi_http::p3::Response, ErrorCode> = store
        .run_concurrent(async move |accessor| service.handle(accessor, wasi_req).await)
        .await??;

    // Wait for request I/O to complete
    if let Err(e) = req_io.await {
        error!(err = ?e, "P3 request I/O error");
    }

    match result {
        Ok(response) => {
            let http_response = response.into_http(&mut store, async { Ok(()) })?;
            Ok(http_response)
        }
        Err(error_code) => {
            error!(?error_code, "P3 HTTP handler returned error");
            let body: P3Body = http_body_util::Empty::new()
                .map_err(|never| match never {})
                .boxed_unsync();
            hyper::Response::builder()
                .status(500)
                .body(body)
                .map_err(anyhow::Error::from)
        }
    }
}

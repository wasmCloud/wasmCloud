//! P3 HTTP handler for WASIP3 components.
//!
//! This module provides the HTTP request handling path for components that
//! target WASIP3's `wasi:http/handler` interface. It uses wasmtime-wasi-http's
//! P3 `ServicePre`/`Service` to invoke the component.

use crate::engine::ctx::SharedCtx;
use crate::observability::FuelConsumptionMeter;
use http_body_util::BodyExt;
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
    // The handler invocation, response conversion, AND body collection must all
    // happen inside run_concurrent since the body stream requires the concurrent
    // runtime to pump data from the component.
    let result: anyhow::Result<hyper::Response<http_body_util::Collected<bytes::Bytes>>> = store
        .run_concurrent(async move |store| {
            let handler_fut = async {
                let result = service.handle(store, wasi_req).await;
                match result {
                    Ok(Ok(response)) => {
                        let http_response =
                            store.with(|s| response.into_http(s, async { Ok(()) }))?;
                        let (parts, body) = http_response.into_parts();
                        let body = body.collect().await.map_err(|e| {
                            anyhow::anyhow!("failed to collect P3 response body: {e:?}")
                        })?;
                        Ok(hyper::Response::from_parts(parts, body))
                    }
                    Ok(Err(error_code)) => {
                        tracing::error!(?error_code, "P3 HTTP handler returned error");
                        let body = http_body_util::Empty::new()
                            .map_err(|never| match never {})
                            .boxed_unsync()
                            .collect()
                            .await
                            .map_err(|e| anyhow::anyhow!("failed to collect error body: {e:?}"))?;
                        Ok(hyper::Response::builder()
                            .status(500)
                            .body(body)
                            .map_err(anyhow::Error::from)?)
                    }
                    Err(e) => Err(anyhow::anyhow!(e).context("P3 handler trap")),
                }
            };
            let io_fut = async {
                if let Err(e) = req_io.await {
                    tracing::error!(err = ?e, "P3 request I/O error");
                }
            };
            let (handler_result, _) = tokio::join!(handler_fut, io_fut);
            handler_result
        })
        .await?;

    // Convert collected body back to a streaming body for the hyper response
    match result {
        Ok(response) => {
            let (parts, collected) = response.into_parts();
            let body: P3Body = http_body_util::Full::new(collected.to_bytes())
                .map_err(|never| match never {})
                .boxed_unsync();
            Ok(hyper::Response::from_parts(parts, body))
        }
        Err(e) => Err(e),
    }
}

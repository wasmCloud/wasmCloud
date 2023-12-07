use super::{Ctx, Instance, InterfaceBindings, InterfaceInstance};

use crate::capability::http::types;
use crate::capability::{IncomingHttp, OutgoingHttp, OutgoingHttpRequest};
use crate::io::AsyncVec;

use core::pin::Pin;
use core::task::Poll;

use std::io::Cursor;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use http_body::Body;
use http_body_util::combinators::BoxBody;
use tokio::io::{AsyncRead, AsyncSeekExt, ReadBuf};
use tokio::sync::{oneshot, Mutex};
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::{self, Table};
use wasmtime_wasi_http::types::{
    HostFutureIncomingResponse, IncomingResponseInternal, OutgoingRequest,
};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

pub mod incoming_http_bindings {
    wasmtime::component::bindgen!({
        world: "incoming-http",
        async: true,
        with: {
           "wasi:http/types@0.2.0-rc-2023-12-05": wasmtime_wasi_http::bindings::http::types,
        },
    });
}

impl WasiHttpView for Ctx {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut Table {
        &mut self.table
    }

    fn send_request(
        &mut self,
        OutgoingRequest {
            use_tls,
            authority,
            request,
            connect_timeout,
            first_byte_timeout,
            between_bytes_timeout,
        }: OutgoingRequest,
    ) -> wasmtime::Result<Resource<HostFutureIncomingResponse>>
    where
        Self: Sized,
    {
        let request = request.map(|body| -> Box<dyn AsyncRead + Send + Sync + Unpin> {
            Box::new(BodyAsyncRead::new(body))
        });
        let handler = self.handler.clone();
        let res = HostFutureIncomingResponse::new(preview2::spawn(async move {
            match OutgoingHttp::handle(
                &handler,
                OutgoingHttpRequest {
                    use_tls,
                    authority,
                    request,
                    connect_timeout,
                    first_byte_timeout,
                    between_bytes_timeout,
                },
            )
            .await
            {
                Ok(resp) => {
                    let resp = resp.map(|body| BoxBody::new(AsyncReadBody::new(body, 1024)));
                    Ok(Ok(IncomingResponseInternal {
                        resp,
                        worker: Arc::new(preview2::spawn(async {})),
                        between_bytes_timeout,
                    }))
                }
                Err(e) => Err(e),
            }
        }));
        let res = self.table().push(res).context("failed to push response")?;
        Ok(res)
    }
}

impl Instance {
    /// Set [`IncomingHttp`] handler for this [Instance].
    pub fn incoming_http(
        &mut self,
        incoming_http: Arc<dyn IncomingHttp + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut().replace_incoming_http(incoming_http);
        self
    }

    /// Set [`OutgoingHttp`] handler for this [Instance].
    pub fn outgoing_http(
        &mut self,
        outgoing_http: Arc<dyn OutgoingHttp + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut().replace_outgoing_http(outgoing_http);
        self
    }

    /// Instantiates and returns a [`InterfaceInstance<incoming_http_bindings::IncomingHttp>`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if incoming HTTP bindings are not exported by the [`Instance`]
    pub async fn into_incoming_http(
        mut self,
    ) -> anyhow::Result<InterfaceInstance<incoming_http_bindings::IncomingHttp>> {
        let bindings = if let Ok((bindings, _)) =
            incoming_http_bindings::IncomingHttp::instantiate_async(
                &mut self.store,
                &self.component,
                &self.linker,
            )
            .await
        {
            InterfaceBindings::Interface(bindings)
        } else {
            self.as_guest_bindings()
                .await
                .map(InterfaceBindings::Guest)
                .context("failed to instantiate `wasi:http/incoming-handler` interface")?
        };
        Ok(InterfaceInstance {
            store: Mutex::new(self.store),
            bindings,
        })
    }
}

struct AsyncReadBody {
    stream: Box<dyn AsyncRead + Sync + Send + Unpin>,
    frame_size: usize,
    end: bool,
}

impl AsyncReadBody {
    pub fn new(
        stream: Box<dyn AsyncRead + Sync + Send + Unpin>,
        frame_size: usize,
    ) -> AsyncReadBody {
        Self {
            stream,
            frame_size,
            end: false,
        }
    }
}

impl Body for AsyncReadBody {
    type Data = Bytes;
    type Error = types::ErrorCode;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let mut bytes = BytesMut::zeroed(self.frame_size);
        let mut buf = ReadBuf::new(&mut bytes);
        match Pin::new(&mut self.stream).poll_read(cx, &mut buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Some(Err(types::ErrorCode::InternalError(Some(
                err.to_string(),
            ))))),
            Poll::Ready(Ok(())) => Poll::Ready({
                let n = buf.filled().len();
                if n == 0 {
                    self.end = true;
                    None
                } else {
                    bytes.truncate(n);
                    Some(Ok(http_body::Frame::data(bytes.freeze())))
                }
            }),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.end
    }
}

struct BodyAsyncRead {
    body: BoxBody<Bytes, types::ErrorCode>,
    buffer: Bytes,
}

impl AsyncRead for BodyAsyncRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.buffer.len() {
            0 => match Pin::new(&mut self.body).poll_frame(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => Poll::Ready(Ok(())),
                Poll::Ready(Some(Ok(frame))) => {
                    if let Ok(mut data) = frame.into_data() {
                        let cap = buf.remaining();
                        if data.len() > cap {
                            self.buffer = data.split_off(cap);
                        }
                        buf.put_slice(&data);
                    }
                    // NOTE: Trailers are not currently supported
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Some(Err(err))) => {
                    Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, err)))
                }
            },
            buffered => {
                let cap = buf.remaining();
                if buffered > cap {
                    let data = self.buffer.split_to(cap);
                    buf.put_slice(&data);
                } else {
                    buf.put_slice(&self.buffer);
                    self.buffer.clear();
                }
                Poll::Ready(Ok(()))
            }
        }
    }
}

impl BodyAsyncRead {
    pub fn new(body: BoxBody<Bytes, types::ErrorCode>) -> Self {
        Self {
            body,
            buffer: Bytes::default(),
        }
    }
}

fn code_to_error(code: types::ErrorCode) -> anyhow::Error {
    match code {
        types::ErrorCode::DnsTimeout => anyhow!("DNS timeout"),
        types::ErrorCode::DnsError(_) => anyhow!("DNS error"),
        types::ErrorCode::DestinationNotFound => anyhow!("destination not found"),
        types::ErrorCode::DestinationUnavailable => anyhow!("destination unavailable"),
        types::ErrorCode::DestinationIpProhibited => anyhow!("destination IP prohibited"),
        types::ErrorCode::DestinationIpUnroutable => anyhow!("destination IP unroutable"),
        types::ErrorCode::ConnectionRefused => anyhow!("connection refused"),
        types::ErrorCode::ConnectionTerminated => anyhow!("connection terminated"),
        types::ErrorCode::ConnectionTimeout => anyhow!("connection timeout"),
        types::ErrorCode::ConnectionReadTimeout => anyhow!("connection read timeout"),
        types::ErrorCode::ConnectionWriteTimeout => anyhow!("connection write timeout"),
        types::ErrorCode::ConnectionLimitReached => anyhow!("connection limit reached"),
        types::ErrorCode::TlsProtocolError => anyhow!("TLS protocol error"),
        types::ErrorCode::TlsCertificateError => anyhow!("TLS certificate error"),
        types::ErrorCode::TlsAlertReceived(_) => anyhow!("TLS alert received"),
        types::ErrorCode::HttpRequestDenied => anyhow!("HTTP request denied"),
        types::ErrorCode::HttpRequestLengthRequired => anyhow!("HTTP request length required"),
        types::ErrorCode::HttpRequestBodySize(_) => anyhow!("HTTP request body size"),
        types::ErrorCode::HttpRequestMethodInvalid => anyhow!("HTTP request method invalid"),
        types::ErrorCode::HttpRequestUriInvalid => anyhow!("HTTP request URI invalid"),
        types::ErrorCode::HttpRequestUriTooLong => anyhow!("HTTP request URI too long"),
        types::ErrorCode::HttpRequestHeaderSectionSize(_) => {
            anyhow!("HTTP request header section size")
        }
        types::ErrorCode::HttpRequestHeaderSize(_) => anyhow!("HTTP request header size"),
        types::ErrorCode::HttpRequestTrailerSectionSize(_) => {
            anyhow!("HTTP request trailer section size")
        }
        types::ErrorCode::HttpRequestTrailerSize(_) => anyhow!("HTTP request trailer size"),
        types::ErrorCode::HttpResponseIncomplete => anyhow!("HTTP response incomplete"),
        types::ErrorCode::HttpResponseHeaderSectionSize(_) => {
            anyhow!("HTTP response header section size")
        }
        types::ErrorCode::HttpResponseHeaderSize(_) => anyhow!("HTTP response header size"),
        types::ErrorCode::HttpResponseBodySize(_) => anyhow!("HTTP response body size"),
        types::ErrorCode::HttpResponseTrailerSectionSize(_) => {
            anyhow!("HTTP response trailer section size")
        }
        types::ErrorCode::HttpResponseTrailerSize(_) => anyhow!("HTTP response trailer size"),
        types::ErrorCode::HttpResponseTransferCoding(_) => {
            anyhow!("HTTP response transfer coding")
        }
        types::ErrorCode::HttpResponseContentCoding(_) => {
            anyhow!("HTTP response content coding")
        }
        types::ErrorCode::HttpResponseTimeout => anyhow!("HTTP response timed out"),
        types::ErrorCode::HttpUpgradeFailed => anyhow!("HTTP upgrade failed"),
        types::ErrorCode::HttpProtocolError => anyhow!("HTTP protocol error"),
        types::ErrorCode::LoopDetected => anyhow!("loop detected"),
        types::ErrorCode::ConfigurationError => anyhow!("configuration error"),
        types::ErrorCode::InternalError(None) => anyhow!("internal error"),
        types::ErrorCode::InternalError(Some(err)) => anyhow!(err).context("internal error"),
    }
}

#[async_trait]
impl IncomingHttp for InterfaceInstance<incoming_http_bindings::IncomingHttp> {
    async fn handle(
        &self,
        request: http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        let mut store = self.store.lock().await;
        match &self.bindings {
            InterfaceBindings::Guest(guest) => {
                let request = wasmcloud_compat::HttpServerRequest::from_http(request)
                    .await
                    .context("failed to convert request")?;
                let request =
                    rmp_serde::to_vec_named(&request).context("failed to encode request")?;
                let mut response = AsyncVec::default();
                match guest
                    .call(
                        &mut store,
                        "HttpServer.HandleRequest",
                        Cursor::new(request),
                        response.clone(),
                    )
                    .await
                    .context("failed to call actor")?
                {
                    Ok(()) => {
                        response
                            .rewind()
                            .await
                            .context("failed to rewind response buffer")?;
                        let response: wasmcloud_compat::HttpResponse =
                            rmp_serde::from_read(&mut response)
                                .context("failed to parse response")?;
                        let response: http::Response<_> =
                            response.try_into().context("failed to convert response")?;
                        Ok(
                            response.map(|body| -> Box<dyn AsyncRead + Send + Sync + Unpin> {
                                Box::new(Cursor::new(body))
                            }),
                        )
                    }
                    Err(err) => bail!(err),
                }
            }
            InterfaceBindings::Interface(bindings) => {
                let ctx = store.data_mut();
                let request = ctx
                    .new_incoming_request(
                        request.map(|stream| BoxBody::new(AsyncReadBody::new(stream, 1024))),
                    )
                    .context("failed to create incoming request")?;
                let (response_tx, mut response_rx) = oneshot::channel();
                let response = ctx
                    .new_response_outparam(response_tx)
                    .context("failed to create response")?;
                bindings
                    .wasi_http_incoming_handler()
                    .call_handle(&mut *store, request, response)
                    .await?;
                match response_rx.try_recv() {
                    Ok(Ok(res)) => {
                        Ok(res.map(|body| -> Box<dyn AsyncRead + Sync + Send + Unpin> {
                            Box::new(BodyAsyncRead::new(body))
                        }))
                    }
                    Ok(Err(err)) => Err(code_to_error(err)),
                    Err(_) => bail!("a response was not set"),
                }
            }
        }
    }
}

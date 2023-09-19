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
           "wasi:http/types@0.2.0-rc-2023-10-18": wasmtime_wasi_http::bindings::http::types,
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
                    Ok(IncomingResponseInternal {
                        resp,
                        worker: preview2::spawn(async { Ok(()) }),
                        between_bytes_timeout,
                    })
                }
                Err(e) => Err(e),
            }
        }));
        let res = self
            .table()
            .push_resource(res)
            .context("failed to push response")?;
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
    type Error = anyhow::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let mut bytes = BytesMut::zeroed(self.frame_size);
        let mut buf = ReadBuf::new(&mut bytes);
        match Pin::new(&mut self.stream).poll_read(cx, &mut buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(err)) => Poll::Ready(Some(Err(anyhow!(err).context("I/O error")))),
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
    body: BoxBody<Bytes, anyhow::Error>,
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
    pub fn new(body: BoxBody<Bytes, anyhow::Error>) -> Self {
        Self {
            body,
            buffer: Bytes::default(),
        }
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
                    Ok(Err(types::Error::InvalidUrl(err))) => {
                        Err(anyhow!(err).context("invalid URL"))
                    }
                    Ok(Err(types::Error::TimeoutError(err))) => {
                        Err(anyhow!(err).context("timeout"))
                    }
                    Ok(Err(types::Error::ProtocolError(err))) => {
                        Err(anyhow!(err).context("protocol error"))
                    }
                    Ok(Err(types::Error::UnexpectedError(err))) => {
                        Err(anyhow!(err).context("unexpected error"))
                    }
                    Err(_) => bail!("a response was not set"),
                }
            }
        }
    }
}

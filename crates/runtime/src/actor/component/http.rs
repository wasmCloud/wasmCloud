use super::{Ctx, Instance, InterfaceBindings, InterfaceInstance, TableResult};

use crate::capability::http::types;
use crate::capability::IncomingHttp;
use crate::io::AsyncVec;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncSeekExt};
use tokio::sync::Mutex;
use tracing::instrument;
use wasmtime_wasi::preview2::pipe::{AsyncReadStream, AsyncWriteStream};
use wasmtime_wasi::preview2::{self, TableStreamExt};

pub mod incoming_http_bindings {
    wasmtime::component::bindgen!({
        world: "incoming-http",
        async: true,
    });
}

struct IncomingRequest {
    method: types::Method,
    path_with_query: Option<String>,
    scheme: Option<types::Scheme>,
    authority: Option<String>,
    headers: types::Headers,
    body: Box<dyn AsyncRead + Sync + Send + Unpin>,
}

struct OutgoingResponse {
    status_code: types::StatusCode,
    headers: types::Headers,
    body: AsyncVec,
}

trait TableKeyValueExt {
    fn push_fields(&mut self, fields: BTreeMap<String, Vec<Vec<u8>>>)
        -> TableResult<types::Fields>;
    fn get_fields(&self, fields: types::Fields) -> TableResult<&BTreeMap<String, Vec<Vec<u8>>>>;
    fn get_fields_mut(
        &mut self,
        fields: types::Fields,
    ) -> TableResult<&mut BTreeMap<String, Vec<Vec<u8>>>>;
    fn delete_fields(
        &mut self,
        fields: types::Fields,
    ) -> TableResult<BTreeMap<String, Vec<Vec<u8>>>>;

    fn push_incoming_request(
        &mut self,
        request: IncomingRequest,
    ) -> TableResult<types::IncomingRequest>;
    fn get_incoming_request(
        &self,
        request: types::IncomingRequest,
    ) -> TableResult<&IncomingRequest>;
    fn delete_incoming_request(
        &mut self,
        request: types::IncomingRequest,
    ) -> TableResult<IncomingRequest>;

    fn new_response_outparam(&mut self) -> TableResult<types::ResponseOutparam>;
    fn get_response_outparam_mut(
        &mut self,
        response: types::ResponseOutparam,
    ) -> TableResult<&mut Option<Result<types::IncomingResponse, types::Error>>>;
    fn delete_response_outparam(
        &mut self,
        response: types::ResponseOutparam,
    ) -> TableResult<Option<Result<types::IncomingResponse, types::Error>>>;

    fn push_outgoing_response(
        &mut self,
        response: OutgoingResponse,
    ) -> TableResult<types::OutgoingResponse>;
    fn get_outgoing_response(
        &self,
        response: types::OutgoingResponse,
    ) -> TableResult<&OutgoingResponse>;
    fn delete_outgoing_response(
        &mut self,
        response: types::OutgoingResponse,
    ) -> TableResult<OutgoingResponse>;
}

impl TableKeyValueExt for preview2::Table {
    fn push_fields(
        &mut self,
        fields: BTreeMap<String, Vec<Vec<u8>>>,
    ) -> TableResult<types::Fields> {
        self.push(Box::new(fields))
    }

    fn get_fields(&self, fields: types::Fields) -> TableResult<&BTreeMap<String, Vec<Vec<u8>>>> {
        self.get(fields)
    }

    fn get_fields_mut(
        &mut self,
        fields: types::Fields,
    ) -> TableResult<&mut BTreeMap<String, Vec<Vec<u8>>>> {
        self.get_mut(fields)
    }

    fn delete_fields(
        &mut self,
        fields: types::Fields,
    ) -> TableResult<BTreeMap<String, Vec<Vec<u8>>>> {
        self.delete(fields)
    }

    fn push_incoming_request(
        &mut self,
        request: IncomingRequest,
    ) -> TableResult<types::IncomingRequest> {
        self.push(Box::new(request))
    }

    fn get_incoming_request(
        &self,
        request: types::IncomingRequest,
    ) -> TableResult<&IncomingRequest> {
        self.get(request)
    }

    fn delete_incoming_request(
        &mut self,
        request: types::IncomingRequest,
    ) -> TableResult<IncomingRequest> {
        self.delete(request)
    }

    fn new_response_outparam(&mut self) -> TableResult<types::ResponseOutparam> {
        self.push(Box::new(
            None::<Result<types::IncomingResponse, types::Error>>,
        ))
    }

    fn get_response_outparam_mut(
        &mut self,
        response: types::ResponseOutparam,
    ) -> TableResult<&mut Option<Result<types::IncomingResponse, types::Error>>> {
        self.get_mut(response)
    }

    fn delete_response_outparam(
        &mut self,
        response: types::ResponseOutparam,
    ) -> TableResult<Option<Result<types::IncomingResponse, types::Error>>> {
        self.delete(response)
    }

    fn push_outgoing_response(
        &mut self,
        response: OutgoingResponse,
    ) -> TableResult<types::OutgoingResponse> {
        self.push(Box::new(response))
    }

    fn get_outgoing_response(
        &self,
        response: types::OutgoingResponse,
    ) -> TableResult<&OutgoingResponse> {
        self.get(response)
    }

    fn delete_outgoing_response(
        &mut self,
        response: types::OutgoingResponse,
    ) -> TableResult<OutgoingResponse> {
        self.delete(response)
    }
}

#[async_trait]
impl types::Host for Ctx {
    async fn drop_fields(&mut self, fields: types::Fields) -> anyhow::Result<()> {
        self.table
            .delete_fields(fields)
            .context("failed to delete fields")?;
        Ok(())
    }
    async fn new_fields(
        &mut self,
        entries: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<types::Fields> {
        let entries =
            entries
                .into_iter()
                .fold(BTreeMap::<_, Vec<_>>::default(), |mut entries, (k, v)| {
                    entries.entry(k).or_default().push(v);
                    entries
                });
        self.table
            .push_fields(entries)
            .context("failed to push fields")
    }
    async fn fields_get(
        &mut self,
        fields: types::Fields,
        name: String,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        let fields = self
            .table
            .get_fields(fields)
            .context("failed to get fields")?;
        let fields = fields.get(&name).context("key does not exist")?;
        Ok(fields.clone())
    }
    async fn fields_set(
        &mut self,
        fields: types::Fields,
        name: String,
        value: Vec<Vec<u8>>,
    ) -> anyhow::Result<()> {
        let fields = self
            .table
            .get_fields_mut(fields)
            .context("failed to get fields")?;
        fields.insert(name, value);
        Ok(())
    }
    async fn fields_delete(&mut self, fields: types::Fields, name: String) -> anyhow::Result<()> {
        let fields = self
            .table
            .get_fields_mut(fields)
            .context("failed to get fields")?;
        fields.remove(&name).context("key not found")?;
        Ok(())
    }
    async fn fields_append(
        &mut self,
        fields: types::Fields,
        name: String,
        value: Vec<u8>,
    ) -> anyhow::Result<()> {
        let fields = self
            .table
            .get_fields_mut(fields)
            .context("failed to get fields")?;
        fields.entry(name).or_default().push(value);
        Ok(())
    }
    async fn fields_entries(
        &mut self,
        fields: types::Fields,
    ) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let fields = self
            .table
            .get_fields(fields)
            .context("failed to get fields")?;
        Ok(fields
            .iter()
            .flat_map(|(k, v)| v.iter().map(|v| (k.clone(), v.clone())).collect::<Vec<_>>())
            .collect())
    }
    async fn fields_clone(&mut self, fields: types::Fields) -> anyhow::Result<types::Fields> {
        let fields = self
            .table
            .get_fields(fields)
            .context("failed to get fields")?;
        let fields = fields.clone();
        self.table
            .push_fields(fields)
            .context("failed to push fields")
    }
    async fn finish_incoming_stream(
        &mut self,
        s: types::IncomingStream,
    ) -> anyhow::Result<Option<types::Trailers>> {
        self.table
            .get_input_stream_mut(s)
            .context("failed to get output stream")?;
        // TODO: Read to end and get trailers
        Ok(None)
    }
    async fn finish_outgoing_stream(&mut self, s: types::OutgoingStream) -> anyhow::Result<()> {
        self.table
            .get_output_stream_mut(s)
            .context("failed to get output stream")?;
        // TODO: Close
        Ok(())
    }
    #[allow(unused)] // TODO: Remove
    async fn finish_outgoing_stream_with_trailers(
        &mut self,
        s: types::OutgoingStream,
        trailers: types::Trailers,
    ) -> anyhow::Result<types::FutureWriteTrailersResult> {
        self.table
            .get_output_stream_mut(s)
            .context("failed to get output stream")?;
        // TODO: Close
        bail!("trailers not supported yet")
    }

    #[allow(unused)] // TODO: Remove
    async fn drop_future_trailers(&mut self, f: types::FutureTrailers) -> anyhow::Result<()> {
        bail!("trailers not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn future_trailers_get(
        &mut self,
        f: types::FutureTrailers,
    ) -> anyhow::Result<Option<Result<types::Trailers, types::Error>>> {
        bail!("trailers not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn listen_to_future_trailers(
        &mut self,
        f: types::FutureTrailers,
    ) -> anyhow::Result<types::Pollable> {
        bail!("trailers not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn drop_future_write_trailers_result(
        &mut self,
        f: types::FutureWriteTrailersResult,
    ) -> anyhow::Result<()> {
        bail!("trailers not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn future_write_trailers_result_get(
        &mut self,
        f: types::FutureWriteTrailersResult,
    ) -> anyhow::Result<Option<Result<(), types::Error>>> {
        bail!("trailers not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn listen_to_future_write_trailers_result(
        &mut self,
        f: types::FutureWriteTrailersResult,
    ) -> anyhow::Result<types::Pollable> {
        bail!("trailers not supported yet")
    }

    async fn drop_incoming_request(
        &mut self,
        request: types::IncomingRequest,
    ) -> anyhow::Result<()> {
        self.table
            .delete_incoming_request(request)
            .context("failed to delete incoming request")?;
        Ok(())
    }
    #[allow(unused)] // TODO: Remove
    async fn drop_outgoing_request(
        &mut self,
        request: types::OutgoingRequest,
    ) -> anyhow::Result<()> {
        bail!("outgoing HTTP not supported yet")
    }
    async fn incoming_request_method(
        &mut self,
        request: types::IncomingRequest,
    ) -> anyhow::Result<types::Method> {
        let IncomingRequest { method, .. } = self
            .table
            .get_incoming_request(request)
            .context("failed to get incoming request")?;
        Ok(method.clone())
    }
    async fn incoming_request_path_with_query(
        &mut self,
        request: types::IncomingRequest,
    ) -> anyhow::Result<Option<String>> {
        let IncomingRequest {
            path_with_query, ..
        } = self
            .table
            .get_incoming_request(request)
            .context("failed to get incoming request")?;
        Ok(path_with_query.clone())
    }
    async fn incoming_request_scheme(
        &mut self,
        request: types::IncomingRequest,
    ) -> anyhow::Result<Option<types::Scheme>> {
        let IncomingRequest { scheme, .. } = self
            .table
            .get_incoming_request(request)
            .context("failed to get incoming request")?;
        Ok(scheme.clone())
    }
    async fn incoming_request_authority(
        &mut self,
        request: types::IncomingRequest,
    ) -> anyhow::Result<Option<String>> {
        let IncomingRequest { authority, .. } = self
            .table
            .get_incoming_request(request)
            .context("failed to get incoming request")?;
        Ok(authority.clone())
    }
    async fn incoming_request_headers(
        &mut self,
        request: types::IncomingRequest,
    ) -> anyhow::Result<types::Headers> {
        let IncomingRequest { headers, .. } = self
            .table
            .get_incoming_request(request)
            .context("failed to get incoming request")?;
        Ok(*headers)
    }
    async fn incoming_request_consume(
        &mut self,
        request: types::IncomingRequest,
    ) -> anyhow::Result<Result<types::IncomingStream, ()>> {
        let IncomingRequest { body, .. } = self
            .table
            .delete_incoming_request(request)
            .context("failed to delete incoming request")?;
        let stream = self
            .table
            .push_input_stream(Box::new(AsyncReadStream::new(body)))
            .context("failed to push input stream")?;
        Ok(Ok(stream))
    }
    #[allow(unused)] // TODO: Remove
    async fn new_outgoing_request(
        &mut self,
        method: types::Method,
        path_with_query: Option<String>,
        scheme: Option<types::Scheme>,
        authority: Option<String>,
        headers: types::Headers,
    ) -> anyhow::Result<Result<types::OutgoingRequest, types::Error>> {
        bail!("outgoing HTTP not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn outgoing_request_write(
        &mut self,
        request: types::OutgoingRequest,
    ) -> anyhow::Result<Result<types::OutgoingStream, ()>> {
        bail!("outgoing HTTP not supported yet")
    }
    async fn drop_response_outparam(
        &mut self,
        response: types::ResponseOutparam,
    ) -> anyhow::Result<()> {
        self.table
            .delete_response_outparam(response)
            .context("failed to delete outgoing response parameter")?;
        Ok(())
    }
    async fn set_response_outparam(
        &mut self,
        param: types::ResponseOutparam,
        response: Result<types::OutgoingResponse, types::Error>,
    ) -> anyhow::Result<Result<(), ()>> {
        let param = self
            .table
            .get_response_outparam_mut(param)
            .context("failed to get outgoing response parameter")?;
        let _ = param.insert(response);
        Ok(Ok(()))
    }

    #[allow(unused)] // TODO: Remove
    async fn drop_incoming_response(
        &mut self,
        response: types::IncomingResponse,
    ) -> anyhow::Result<()> {
        bail!("outgoing HTTP not supported yet")
    }
    async fn drop_outgoing_response(
        &mut self,
        response: types::OutgoingResponse,
    ) -> anyhow::Result<()> {
        self.table
            .delete_outgoing_response(response)
            .context("failed to delete outgoing response")?;
        Ok(())
    }
    #[allow(unused)] // TODO: Remove
    async fn incoming_response_status(
        &mut self,
        response: types::IncomingResponse,
    ) -> anyhow::Result<types::StatusCode> {
        bail!("outgoing HTTP not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn incoming_response_headers(
        &mut self,
        response: types::IncomingResponse,
    ) -> anyhow::Result<types::Headers> {
        bail!("outgoing HTTP not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn incoming_response_consume(
        &mut self,
        response: types::IncomingResponse,
    ) -> anyhow::Result<Result<types::IncomingStream, ()>> {
        bail!("outgoing HTTP not supported yet")
    }
    async fn new_outgoing_response(
        &mut self,
        status_code: types::StatusCode,
        headers: types::Headers,
    ) -> anyhow::Result<Result<types::OutgoingResponse, types::Error>> {
        let response = self
            .table
            .push_outgoing_response(OutgoingResponse {
                status_code,
                headers,
                body: AsyncVec::default(),
            })
            .context("failed to push fields")?;
        Ok(Ok(response))
    }
    async fn outgoing_response_write(
        &mut self,
        response: types::OutgoingResponse,
    ) -> anyhow::Result<Result<types::OutgoingStream, ()>> {
        let OutgoingResponse { body, .. } = self
            .table
            .get_outgoing_response(response)
            .context("failed to get outgoing response")?;
        let stream = self
            .table
            .push_output_stream(Box::new(AsyncWriteStream::new(1 << 16, body.clone())))
            .context("failed to push output stream")?;
        Ok(Ok(stream))
    }

    #[allow(unused)] // TODO: Remove
    async fn drop_future_incoming_response(
        &mut self,
        f: types::FutureIncomingResponse,
    ) -> anyhow::Result<()> {
        bail!("outgoing HTTP not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn future_incoming_response_get(
        &mut self,
        f: types::FutureIncomingResponse,
    ) -> anyhow::Result<Option<Result<types::IncomingResponse, types::Error>>> {
        bail!("outgoing HTTP not supported yet")
    }
    #[allow(unused)] // TODO: Remove
    async fn listen_to_future_incoming_response(
        &mut self,
        f: types::FutureIncomingResponse,
    ) -> anyhow::Result<types::Pollable> {
        bail!("outgoing HTTP not supported yet")
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

#[async_trait]
impl IncomingHttp for InterfaceInstance<incoming_http_bindings::IncomingHttp> {
    #[allow(unused)] // TODO: Remove
    #[instrument(skip_all)]
    async fn handle(
        &self,
        request: http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        let mut store = self.store.lock().await;
        match &self.bindings {
            InterfaceBindings::Guest(guest) => {
                let request = wasmcloud_compat::HttpRequest::from_http(request)
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
                let (
                    http::request::Parts {
                        method,
                        uri,
                        headers,
                        ..
                    },
                    body,
                ) = request.into_parts();
                let method = match method.as_str() {
                    "GET" => types::Method::Get,
                    "HEAD" => types::Method::Head,
                    "POST" => types::Method::Post,
                    "PUT" => types::Method::Put,
                    "DELETE" => types::Method::Delete,
                    "CONNECT" => types::Method::Connect,
                    "OPTIONS" => types::Method::Options,
                    "TRACE" => types::Method::Trace,
                    "PATCH" => types::Method::Patch,
                    method => bail!("unknown method `{method}`"),
                };
                let path_with_query = uri.path_and_query().map(http::uri::PathAndQuery::as_str);
                let scheme = uri.scheme_str().map(|scheme| match scheme {
                    "http" => types::Scheme::Http,
                    "https" => types::Scheme::Https,
                    other => types::Scheme::Other(other.to_string()),
                });
                let authority = uri.authority().map(http::uri::Authority::as_str);
                let mut data = store.data_mut();
                let headers = headers
                    .into_iter()
                    .map(|(name, value)| {
                        let name = name.context("invalid header name")?;
                        Ok((name.as_str().into(), vec![value.as_bytes().into()]))
                    })
                    .collect::<anyhow::Result<_>>()
                    .context("failed to parse headers")?;
                let headers = data
                    .table
                    .push_fields(headers)
                    .context("failed to push headers")?;
                let request = data
                    .table
                    .push(Box::new(IncomingRequest {
                        method,
                        path_with_query: path_with_query.map(Into::into),
                        scheme,
                        authority: authority.map(Into::into),
                        body,
                        headers,
                    }))
                    .context("failed to push request to table")?;
                let response = data
                    .table
                    .new_response_outparam()
                    .context("failed to push response to table")?;
                bindings
                    .wasi_http_incoming_handler()
                    .call_handle(&mut *store, request, response)
                    .await?;
                let data = store.data_mut();
                match data
                    .table
                    .delete_response_outparam(response)
                    .context("failed to delete outgoing response parameter")?
                {
                    None => bail!("response not set"),
                    Some(Ok(response)) => {
                        let OutgoingResponse {
                            status_code,
                            headers,
                            mut body,
                        } = data
                            .table
                            .delete_outgoing_response(response)
                            .context("failed to delete outgoing response")?;
                        let headers = data
                            .table
                            .delete_fields(headers)
                            .context("failed to delete headers")?;
                        let res = http::Response::builder().status(status_code);
                        let res = headers.into_iter().fold(res, |res, (name, mut values)| {
                            if let Some(value) = values.pop() {
                                res.header(name, value)
                            } else {
                                res
                            }
                        });
                        body.rewind()
                            .await
                            .context("failed to rewind response body")?;
                        let body: Box<dyn AsyncRead + Send + Sync + Unpin> = Box::new(body);
                        res.body(body).context("failed to create response")
                    }
                    Some(Err(types::Error::InvalidUrl(err))) => {
                        Err(anyhow!(err).context("invalid URL"))
                    }
                    Some(Err(types::Error::TimeoutError(err))) => {
                        Err(anyhow!(err).context("timeout"))
                    }
                    Some(Err(types::Error::ProtocolError(err))) => {
                        Err(anyhow!(err).context("protocol error"))
                    }
                    Some(Err(types::Error::UnexpectedError(err))) => {
                        Err(anyhow!(err).context("unexpected error"))
                    }
                }
            }
        }
    }
}

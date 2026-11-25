//! `client.send`: the managed-class pipeline.
//!
//! The request is checked against the binding, the router is asked, and only
//! then is the credential written in. It leaves through the host's network
//! path with the allowlist, substitution and local routes skipped, since the
//! binding already decided all three, and the post-DNS address gate still
//! applies. Bodies stream in both directions.
//!
//! The credential is scrubbed from the response head and trailers in every
//! spelling, percent-encoded ones included, as egress secrets are. Other
//! encodings such as base64 are not recognized. Bodies are not scanned,
//! so an upstream that echoes the credential in a body hands it to the guest.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::task::{Context, Poll, Waker, ready};

use bytes::Bytes;
use http::uri::Scheme;
use http::{HeaderName, HeaderValue, header};
use http_body_util::BodyExt as _;
use hyper::body::{Frame, SizeHint};
use tokio::sync::oneshot;
use wasmtime::component::{Accessor, Resource};
use wasmtime_wasi_http::p3::bindings::http::types::{
    DnsErrorPayload, ErrorCode as HttpErrorCode, TlsAlertReceivedPayload,
};
use wasmtime_wasi_http::p3::{Request, Response};
use wasmtime_wasi_http::{WasiBody, WasiHttpView};

use super::bindings::wasmcloud::secrets::http_client::ErrorCode as SendError;
use super::bindings::wasmcloud::secrets::types::DeniedReason;
use super::{AUDIT_ID_HEADER, Audit, BoundClient, BrokerOperation, BrokerOutcome, reason_name};
use crate::engine::ctx::SharedCtx;
use crate::host::dpop::{DPOP, wants_nonce};
use crate::host::egress_credential::{
    CarriesSecret, EgressCaller, EgressCredentialSource, OVERRIDE_HEADERS, PLACEHOLDER_MARKER,
    SecretRoute, Substituted, is_ambiguous,
};
use crate::host::http::{BrokeredRequest, HostHandler, RequestIoFuture, SendFuture};
use crate::host::token_exchange::{TokenExchange, TokenType};

/// Headers proxies take as the client's address.
const CLIENT_ADDRESS_HEADERS: &[&str] = &[
    "x-real-ip",
    "true-client-ip",
    "client-ip",
    "x-client-ip",
    "x-cluster-client-ip",
    "cf-connecting-ip",
    "fastly-client-ip",
    "x-originating-ip",
];

/// What a presented credential is replaced with in a response.
const REDACTED: &[u8] = b"[redacted]";

fn carries_placeholder(bytes: &[u8]) -> bool {
    bytes
        .windows(PLACEHOLDER_MARKER.len())
        .any(|w| w == PLACEHOLDER_MARKER.as_bytes())
}

/// How the bound credential is written into a request.
#[derive(Clone)]
pub enum Presenter {
    /// `Authorization: Bearer <value>`.
    Bearer(Arc<dyn EgressCredentialSource>),
    /// `<name>: <value>`, for an API key header.
    Header(HeaderName, Arc<dyn EgressCredentialSource>),
    /// An exchanged token, as Bearer or with a DPoP proof per its type.
    Token(Arc<TokenExchange>),
}

impl Presenter {
    fn header(&self) -> &HeaderName {
        match self {
            Presenter::Header(name, _) => name,
            _ => &header::AUTHORIZATION,
        }
    }
}

/// What one labeled `client` import may reach, and with what.
#[derive(Clone)]
pub struct ClientBinding {
    destinations: Vec<(String, u16)>,
    routes: Vec<SecretRoute>,
    presenter: Presenter,
}

impl ClientBinding {
    pub fn new(presenter: Presenter) -> Self {
        Self {
            destinations: Vec::new(),
            routes: Vec::new(),
            presenter,
        }
    }

    /// Allow `https://host:port`, matched exactly.
    pub fn to(mut self, host: &str, port: u16) -> Self {
        self.destinations.push((host.to_ascii_lowercase(), port));
        self
    }

    /// Restrict to these routes. With none, any method and path is allowed.
    pub fn on(mut self, route: SecretRoute) -> Self {
        self.routes.push(route);
        self
    }

    /// Check `request` against the binding and rewrite its destination to the
    /// one form the credential and any proof are bound to.
    fn authorize<B>(&self, request: &mut http::Request<B>) -> Result<(), DeniedReason> {
        let method = request.method().as_str();
        if method.eq_ignore_ascii_case("TRACE") || method.eq_ignore_ascii_case("CONNECT") {
            return Err(DeniedReason::OperationNotAllowed);
        }
        let uri = request.uri();
        let authority = uri
            .authority()
            .filter(|_| uri.scheme() == Some(&Scheme::HTTPS))
            .filter(|a| !a.as_str().contains('@'))
            .ok_or(DeniedReason::DestinationNotAllowed)?;
        let host = authority.host().to_ascii_lowercase();
        let port = authority.port_u16().unwrap_or(443);
        if !self.destinations.contains(&(host.clone(), port)) {
            return Err(DeniedReason::DestinationNotAllowed);
        }
        if !self.routes.is_empty()
            && (is_ambiguous(uri.path())
                || !self
                    .routes
                    .iter()
                    .any(|r| r.allows(request.method(), uri.path())))
        {
            return Err(DeniedReason::OperationNotAllowed);
        }
        let overrides_method = uri.query().is_some_and(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .any(|(k, _)| k.eq_ignore_ascii_case("_method"))
        });
        if overrides_method {
            return Err(DeniedReason::ReservedHeader);
        }
        for (name, value) in request.headers() {
            if (self.owns(name) && name != header::HOST)
                || (name == header::HOST && value.as_bytes() != authority.as_str().as_bytes())
                || carries_placeholder(value.as_bytes())
            {
                return Err(DeniedReason::ReservedHeader);
            }
        }

        let canonical = if port == 443 {
            host
        } else {
            format!("{host}:{port}")
        };
        let mut parts = uri.clone().into_parts();
        parts.authority = Some(
            canonical
                .parse()
                .map_err(|_| DeniedReason::DestinationNotAllowed)?,
        );
        *request.uri_mut() =
            http::Uri::from_parts(parts).map_err(|_| DeniedReason::DestinationNotAllowed)?;
        if request.headers().contains_key(header::HOST) {
            let host = HeaderValue::try_from(canonical)
                .map_err(|_| DeniedReason::DestinationNotAllowed)?;
            request.headers_mut().insert(header::HOST, host);
        }
        Ok(())
    }

    /// Headers and trailers the broker writes or that would redirect the
    /// request once the credential is on it.
    fn owns(&self, name: &HeaderName) -> bool {
        name == header::AUTHORIZATION
            || name == header::PROXY_AUTHORIZATION
            || name == header::HOST
            || name == DPOP
            || name == self.presenter.header()
            || name == header::FORWARDED
            || name == header::MAX_FORWARDS
            || name == header::VIA
            || CLIENT_ADDRESS_HEADERS.contains(&name.as_str())
            || name.as_str().starts_with("x-forwarded-")
            || OVERRIDE_HEADERS.contains(&name.as_str())
    }

    /// Write the credential in, returning what scrubs it from the response.
    fn present<B>(&self, request: &mut http::Request<B>) -> Result<Substituted, DeniedReason> {
        let unavailable = |_| DeniedReason::CredentialUnavailable;
        let (name, secret, value, proof) = match &self.presenter {
            Presenter::Bearer(source) => {
                let secret = source.current().map_err(unavailable)?;
                let value = prefixed(b"Bearer ", &secret);
                (header::AUTHORIZATION, secret, value, None)
            }
            Presenter::Header(name, source) => {
                let secret = source.current().map_err(unavailable)?;
                let value = HeaderValue::from_bytes(&secret).ok();
                (name.clone(), secret, value, None)
            }
            Presenter::Token(exchange) => {
                let token = exchange.current().map_err(unavailable)?;
                match (exchange.token_type(), exchange.dpop_key()) {
                    (Some(TokenType::Dpop), Some(key)) => {
                        let proof = key
                            .proof(request.method(), request.uri(), Some(&token))
                            .map_err(|_| DeniedReason::CredentialUnavailable)?;
                        let value = prefixed(b"DPoP ", &token);
                        (header::AUTHORIZATION, token, value, Some(proof))
                    }
                    (Some(TokenType::Bearer), None) => {
                        let value = prefixed(b"Bearer ", &token);
                        (header::AUTHORIZATION, token, value, None)
                    }
                    _ => return Err(DeniedReason::CredentialUnavailable),
                }
            }
        };
        let mut value = value.ok_or(DeniedReason::CredentialUnavailable)?;
        let scrub = Substituted::presented(&[value.as_bytes(), &secret], REDACTED);
        value.set_sensitive(true);
        let headers = request.headers_mut();
        headers.insert(name, value);
        if let Some(mut proof) = proof {
            proof.set_sensitive(true);
            headers.insert(DPOP, proof);
        }
        request.extensions_mut().insert(CarriesSecret);
        request.extensions_mut().insert(BrokeredRequest::new());
        Ok(scrub)
    }

    fn dpop_key(&self) -> Option<&Arc<crate::host::dpop::DpopKey>> {
        match &self.presenter {
            Presenter::Token(exchange) if exchange.token_type() == Some(TokenType::Dpop) => {
                exchange.dpop_key()
            }
            _ => None,
        }
    }
}

fn prefixed(prefix: &[u8], value: &[u8]) -> Option<HeaderValue> {
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(prefix.len() + value.len()));
    bytes.extend_from_slice(prefix);
    bytes.extend_from_slice(value);
    HeaderValue::from_bytes(&bytes).ok()
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The response body. It owns the I/O task and the upload's lifeline, so
/// dropping the response ends the exchange, upload included, and it scrubs
/// the credential from trailers.
struct Held {
    body: WasiBody,
    scrub: Arc<Substituted>,
    _task: Option<AbortOnDrop>,
    _upload: oneshot::Sender<()>,
}

impl hyper::body::Body for Held {
    type Data = Bytes;
    type Error = wasmtime_wasi_http::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = &mut *self;
        Pin::new(&mut this.body)
            .poll_frame(cx)
            .map_ok(|frame| match frame.into_trailers() {
                Ok(mut trailers) => {
                    this.scrub.revert(&mut trailers);
                    Frame::trailers(trailers)
                }
                Err(frame) => frame,
            })
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

const GUEST_BODY_FAILED: &str = "the request body failed";
const RESPONSE_DROPPED: &str = "the response was dropped";

/// How the request body ended, as [`Upload`] saw it.
const BODY_OPEN: u8 = 0;
const BODY_CLEAN: u8 = 1;
const BODY_TRAILERS: u8 = 2;

/// What [`Upload`] saw of the request body, read after the response head.
#[derive(Default)]
struct BodyState {
    ended: AtomicU8,
    bytes: AtomicU64,
}

impl BodyState {
    /// Ended with no data and no trailers, so the request can be sent again
    /// as it stands.
    fn replayable(&self) -> bool {
        self.ended.load(Ordering::Acquire) == BODY_CLEAN && self.bytes.load(Ordering::Acquire) == 0
    }
}

/// The request body as it leaves: trailers the broker owns are removed, a
/// guest's error becomes the broker's own so a code the guest picked is never
/// taken as the host's verdict, and the upload fails once the response is
/// dropped.
struct Upload {
    body: WasiBody,
    binding: Arc<ClientBinding>,
    response: Option<oneshot::Receiver<()>>,
    state: Arc<BodyState>,
}

impl Upload {
    fn new(body: WasiBody, binding: Arc<ClientBinding>, response: oneshot::Receiver<()>) -> Self {
        Self {
            body,
            binding,
            response: Some(response),
            state: Arc::default(),
        }
    }
}

impl hyper::body::Body for Upload {
    type Data = Bytes;
    type Error = wasmtime_wasi_http::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = &mut *self;
        let dropped = || {
            Poll::Ready(Some(Err(HttpError::InternalError(Some(
                RESPONSE_DROPPED.into(),
            )))))
        };
        let Some(response) = &mut this.response else {
            return dropped();
        };
        if Pin::new(response).poll(cx).is_ready() {
            this.response = None;
            return dropped();
        }
        let frame = match ready!(Pin::new(&mut this.body).poll_frame(cx)) {
            None => {
                let _ = this.state.ended.compare_exchange(
                    BODY_OPEN,
                    BODY_CLEAN,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                return Poll::Ready(None);
            }
            Some(Err(_)) => {
                return Poll::Ready(Some(Err(HttpError::InternalError(Some(
                    GUEST_BODY_FAILED.into(),
                )))));
            }
            Some(Ok(frame)) => frame,
        };
        let mut trailers = match frame.into_trailers() {
            Ok(trailers) => trailers,
            Err(frame) => {
                if let Some(data) = frame.data_ref() {
                    let n = u64::try_from(data.len()).unwrap_or(u64::MAX);
                    this.state.bytes.fetch_add(n, Ordering::AcqRel);
                }
                return Poll::Ready(Some(Ok(frame)));
            }
        };
        this.state.ended.store(BODY_TRAILERS, Ordering::Release);
        let owned: Vec<HeaderName> = trailers
            .iter()
            .filter(|(name, value)| {
                this.binding.owns(name) || carries_placeholder(value.as_bytes())
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in owned {
            trailers.remove(name);
        }
        Poll::Ready(Some(Ok(Frame::trailers(trailers))))
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

type HttpError = wasmtime_wasi_http::Error;

/// What became of the request once the broker was done with it.
enum Transmit {
    Settled(Result<(), HttpError>),
    Running(oneshot::Receiver<Result<(), HttpError>>),
}

/// The guest's view of its request body: an error unless the request left and
/// its I/O finished.
async fn transmit_result(rx: oneshot::Receiver<Transmit>) -> Result<(), HttpError> {
    match rx.await {
        Ok(Transmit::Settled(result)) => result,
        Ok(Transmit::Running(io)) => io
            .await
            .unwrap_or(Err(HttpError::InternalError(Some("canceled".into())))),
        Err(_) => Err(HttpError::HttpRequestDenied),
    }
}

type ResponseIo = Box<dyn std::future::Future<Output = Result<(), HttpError>> + Send + 'static>;

/// The guest's response-body outcome, handed to the transport once known.
fn response_io(rx: oneshot::Receiver<ResponseIo>) -> RequestIoFuture {
    Box::new(async move {
        match rx.await {
            Ok(fut) => Box::into_pin(fut).await,
            Err(_) => Ok(()),
        }
    })
}

struct Attempt {
    send: SendFuture,
    response_tx: oneshot::Sender<ResponseIo>,
}

/// The request's head, to send again with a fresh DPoP nonce if its body
/// turns out to have been empty.
struct Replay(http::request::Parts);

/// Who sends the request and on whose behalf.
struct Issuer {
    handler: Arc<dyn HostHandler>,
    workload_id: Arc<str>,
    namespace: Arc<str>,
    name: Arc<str>,
    component_id: Arc<str>,
    options: Option<wasmtime_wasi_http::RequestOptions>,
}

impl Issuer {
    fn caller(&self) -> EgressCaller<'_> {
        EgressCaller {
            workload_id: &self.workload_id,
            namespace: &self.namespace,
            name: &self.name,
            component_id: Some(&*self.component_id).filter(|id| !id.is_empty()),
        }
    }

    /// The router's say, asked before the credential is written in.
    fn allow(&self, request: &http::Request<WasiBody>) -> Result<(), DeniedReason> {
        self.handler
            .allow_brokered_request(self.caller(), request)
            .map_err(|e| {
                tracing::warn!(workload_id = %self.workload_id, err = %e, "brokered request refused by the router");
                DeniedReason::NotEntitled
            })
    }

    fn issue(&self, request: http::Request<WasiBody>) -> Attempt {
        let (response_tx, response_rx) = oneshot::channel();
        Attempt {
            send: self.handler.outgoing_request_from(
                self.caller(),
                request,
                self.options,
                response_io(response_rx),
                &[],
            ),
            response_tx,
        }
    }
}

/// The guest's request-body outcome. Once the request is issued, dropping
/// this unsettled, as a canceled `send` does, reports a cancel.
struct Settle(Option<oneshot::Sender<Transmit>>);

impl Settle {
    fn send(mut self, transmit: Transmit) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(transmit);
        }
    }
}

impl Drop for Settle {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(Transmit::Settled(Err(HttpError::InternalError(Some(
                "canceled".into(),
            )))));
        }
    }
}

/// Why nothing was sent.
enum Unsent {
    Denied(DeniedReason),
    Trap(wasmtime::Error),
}

struct Ready {
    first: Attempt,
    replay: Option<Replay>,
    issuer: Issuer,
    state: Arc<BodyState>,
    scrub: Substituted,
    uri: http::Uri,
}

fn denied(reason: DeniedReason) -> SendError {
    SendError::Denied(reason)
}

/// The URI a guest request names, for audit before it is converted.
fn named_uri(request: &Request) -> http::Uri {
    let mut builder = http::Uri::builder();
    if let Some(scheme) = &request.scheme {
        builder = builder.scheme(scheme.clone());
    }
    if let Some(authority) = &request.authority {
        builder = builder.authority(authority.clone());
    }
    builder
        .path_and_query(request.path_with_query.as_ref().map_or("/", |p| p.as_str()))
        .build()
        .unwrap_or_default()
}

pub(super) async fn send(
    accessor: &Accessor<SharedCtx, SharedCtx>,
    bound: &BoundClient,
    request: Resource<Request>,
) -> wasmtime::Result<Result<Resource<Response>, SendError>> {
    let audit_id = crate::ids::now_v7().to_string();
    let binding = &bound.inner;
    let (transmit_tx, transmit_rx) = oneshot::channel::<Transmit>();
    let (upload_tx, upload_rx) = oneshot::channel::<()>();
    let unread = http::Method::from_bytes(b"-").unwrap_or_default();
    let op = Arc::new(std::sync::Mutex::new((unread, http::Uri::default())));
    let note = |seen| *op.lock().unwrap_or_else(|e| e.into_inner()) = seen;
    let audit = {
        let (bound, audit_id, op) = (Arc::clone(bound), audit_id.clone(), Arc::clone(&op));
        Audit::new(move |outcome| {
            let (method, uri) = &*op.lock().unwrap_or_else(|e| e.into_inner());
            let operation = BrokerOperation::Send {
                method,
                authority: uri.authority().map_or("", |a| a.as_str()),
                path: uri.path(),
            };
            bound.audit(&audit_id, operation, outcome);
        })
    };

    let prepared = accessor.with(|mut store| -> wasmtime::Result<_> {
        let request = store.get().table.delete(request)?;
        note((request.method.clone(), named_uri(&request)));
        let (request, options) = match request.into_http(&mut store, transmit_result(transmit_rx)) {
            Ok(converted) => converted,
            Err(e) => {
                let unsent = match e.downcast() {
                    Ok(code) => Unsent::Denied(DeniedReason::Other(Some(format!(
                        "invalid request: {code:?}"
                    )))),
                    Err(trap) => Unsent::Trap(trap),
                };
                return Ok(Err(unsent));
            }
        };
        let (parts, body) = request.into_parts();
        let upload = Upload::new(body, Arc::clone(binding), upload_rx);
        let state = Arc::clone(&upload.state);
        let mut request = http::Request::from_parts(parts, upload.boxed_unsync());
        let ctx = &store.get().ctx;
        let Some(handler) = ctx
            .egress_handler()
            .ok()
            .filter(|h| h.sends_brokered_requests())
        else {
            return Ok(Err(Unsent::Denied(DeniedReason::NotSupported)));
        };
        let (namespace, name) = ctx.egress_names();
        let issuer = Issuer {
            handler,
            workload_id: ctx.workload_id.clone(),
            namespace,
            name,
            component_id: Arc::clone(&bound.caller.component_id),
            options: options.as_deref().copied(),
        };
        let checked = binding
            .authorize(&mut request)
            .and_then(|()| issuer.allow(&request))
            .and_then(|()| binding.present(&mut request));
        let scrub = match checked {
            Ok(scrub) => scrub,
            Err(reason) => return Ok(Err(Unsent::Denied(reason))),
        };
        let uri = request.uri().clone();
        note((request.method().clone(), uri.clone()));
        let replay = binding.dpop_key().map(|_| {
            let (mut parts, ()) = http::Request::new(()).into_parts();
            parts.method = request.method().clone();
            parts.uri = request.uri().clone();
            parts.headers = request.headers().clone();
            Replay(parts)
        });
        let first = issuer.issue(request);
        Ok(Ok(Ready {
            first,
            replay,
            issuer,
            state,
            scrub,
            uri,
        }))
    });

    let Ready {
        first: mut attempt,
        replay,
        issuer,
        state,
        mut scrub,
        uri,
    } = match prepared.unwrap_or_else(|trap| Err(Unsent::Trap(trap))) {
        Ok(ready) => ready,
        Err(Unsent::Denied(reason)) => {
            audit.record(BrokerOutcome::Denied(reason_name(&reason)));
            return Ok(Err(denied(reason)));
        }
        Err(Unsent::Trap(trap)) => {
            audit.record(BrokerOutcome::Failed("the request could not be read"));
            return Err(trap);
        }
    };
    let settle = Settle(Some(transmit_tx));

    let mut outcome = Box::into_pin(attempt.send).await;
    let mut retried = false;
    let challenged = match (binding.dpop_key(), &outcome) {
        (Some(key), Ok((response, _))) => {
            key.observe(&uri, response.headers());
            wants_nonce(response.status(), response.headers())
        }
        _ => false,
    };
    if let Some(replay) = replay.filter(|_| challenged && state.replayable()) {
        let mut request = http::Request::from_parts(
            replay.0,
            http_body_util::Empty::new()
                .map_err(|never: std::convert::Infallible| match never {})
                .boxed_unsync(),
        );
        request.headers_mut().remove(header::AUTHORIZATION);
        request.headers_mut().remove(DPOP);
        if let Ok(presented) = issuer
            .allow(&request)
            .and_then(|()| binding.present(&mut request))
        {
            scrub = presented;
            attempt = issuer.issue(request);
            retried = true;
            outcome = Box::into_pin(attempt.send).await;
        }
    }

    let (mut response, io) = match outcome {
        Ok(sent) => sent,
        Err(e) => {
            let detail = e.to_string();
            let code = match classify(&e) {
                SendError::Denied(reason) if !retried => {
                    audit.record(BrokerOutcome::Denied(reason_name(&reason)));
                    SendError::Denied(reason)
                }
                SendError::Transport(code) if !retried => {
                    audit.record(BrokerOutcome::Failed(&detail));
                    SendError::Transport(code)
                }
                SendError::Denied(_) | SendError::Transport(_) => {
                    audit.record(BrokerOutcome::Indeterminate(&detail));
                    SendError::Indeterminate(Some(detail.clone()))
                }
                code => {
                    audit.record(BrokerOutcome::Indeterminate(&detail));
                    code
                }
            };
            settle.send(Transmit::Settled(Err(HttpError::InternalError(Some(
                detail,
            )))));
            return Ok(Err(code));
        }
    };
    scrub.revert(response.headers_mut());
    if let Ok(value) = HeaderValue::try_from(audit_id.as_str()) {
        response.headers_mut().insert(AUDIT_ID_HEADER, value);
    }

    let mut io = Box::into_pin(io);
    let finished = match io.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => Some(result),
        Poll::Pending => None,
    };
    let (task, settled) = match finished {
        Some(result) => {
            let failed = result.as_ref().err().map(ToString::to_string);
            settle.send(Transmit::Settled(result));
            (None, Some((audit, failed)))
        }
        None => {
            let (tx, rx) = oneshot::channel();
            settle.send(Transmit::Running(rx));
            let task = tokio::spawn(async move {
                let result = io.await;
                match &result {
                    Ok(()) => audit.record(BrokerOutcome::Completed),
                    Err(e) => audit.record(BrokerOutcome::Indeterminate(&e.to_string())),
                }
                let _ = tx.send(result);
            });
            (Some(AbortOnDrop(task)), None)
        }
    };
    let scrub = Arc::new(scrub);
    let response = response.map(|body| {
        Held {
            body,
            scrub,
            _task: task,
            _upload: upload_tx,
        }
        .boxed_unsync()
    });

    let response_tx = attempt.response_tx;
    let resource = accessor.with(|mut store| {
        let (response, response_io) = Response::from_http(store.data_mut().http().hooks, response);
        let _ = response_tx.send(Box::new(response_io));
        store.get().table.push(response)
    })?;
    if let Some((audit, failed)) = settled {
        audit.record(match &failed {
            Some(detail) => BrokerOutcome::Indeterminate(detail),
            None => BrokerOutcome::Completed,
        });
    }
    Ok(Ok(resource))
}

/// Refused before anything was sent, nothing accepted, or unknown.
fn classify(e: &HttpError) -> SendError {
    use wasmtime_wasi_http::Error as E;
    let transport = SendError::Transport;
    match e {
        E::HttpRequestDenied => denied(DeniedReason::NotEntitled),
        E::DestinationIpProhibited => denied(DeniedReason::ResolvedAddressNotAllowed),
        E::DnsTimeout => transport(HttpErrorCode::DnsTimeout),
        E::DnsError { rcode, info_code } => transport(HttpErrorCode::DnsError(DnsErrorPayload {
            rcode: rcode.clone(),
            info_code: *info_code,
        })),
        E::DestinationNotFound => transport(HttpErrorCode::DestinationNotFound),
        E::DestinationUnavailable => transport(HttpErrorCode::DestinationUnavailable),
        E::DestinationIpUnroutable => transport(HttpErrorCode::DestinationIpUnroutable),
        E::Connect(_) | E::ConnectionRefused => transport(HttpErrorCode::ConnectionRefused),
        E::ConnectionTimeout => transport(HttpErrorCode::ConnectionTimeout),
        E::ConnectionLimitReached => transport(HttpErrorCode::ConnectionLimitReached),
        E::Tls(_) | E::TlsProtocolError => transport(HttpErrorCode::TlsProtocolError),
        E::TlsCertificateError => transport(HttpErrorCode::TlsCertificateError),
        E::TlsAlertReceived {
            alert_id,
            alert_message,
        } => transport(HttpErrorCode::TlsAlertReceived(TlsAlertReceivedPayload {
            alert_id: *alert_id,
            alert_message: alert_message.clone(),
        })),
        other => SendError::Indeterminate(Some(other.to_string())),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::host::egress_credential::LeasedSource;

    fn source(value: &[u8]) -> Arc<dyn EgressCredentialSource> {
        let lease = LeasedSource::default();
        lease.renew(
            zeroize::Zeroizing::new(value.to_vec()),
            std::time::Duration::from_secs(60),
        );
        Arc::new(lease)
    }

    fn binding() -> ClientBinding {
        ClientBinding::new(Presenter::Bearer(source(b"tok")))
            .to("API.example.com", 443)
            .on(SecretRoute::new(Some(http::Method::GET), "/v1/repos").unwrap())
    }

    fn named(result: Result<(), DeniedReason>) -> Result<(), &'static str> {
        result.map_err(|reason| reason_name(&reason))
    }

    fn request(uri: &str) -> http::Request<()> {
        http::Request::get(uri).body(()).unwrap()
    }

    #[test]
    fn only_the_bound_destination_and_routes_are_allowed() {
        let b = binding();
        assert_eq!(
            named(b.authorize(&mut request("https://api.example.com/v1/repos/x"))),
            Ok(())
        );
        assert_eq!(
            named(b.authorize(&mut request("http://api.example.com/v1/repos"))),
            Err("destination-not-allowed")
        );
        assert_eq!(
            named(b.authorize(&mut request("https://api.example.com:8443/v1/repos"))),
            Err("destination-not-allowed")
        );
        assert_eq!(
            named(b.authorize(&mut request("https://evil.example.com/v1/repos"))),
            Err("destination-not-allowed")
        );
        assert_eq!(
            named(b.authorize(&mut request("https://api.example.com/v1/reposx"))),
            Err("operation-not-allowed")
        );
        assert_eq!(
            named(b.authorize(&mut request("https://api.example.com/v1/repos/../admin"))),
            Err("operation-not-allowed")
        );
        let lowercase = |m: &[u8]| http::Method::from_bytes(m).unwrap();
        for method in [
            http::Method::TRACE,
            http::Method::CONNECT,
            lowercase(b"trace"),
            lowercase(b"Connect"),
        ] {
            let mut req = request("https://api.example.com/v1/repos");
            *req.method_mut() = method;
            assert_eq!(named(b.authorize(&mut req)), Err("operation-not-allowed"));
        }
        let mut post = http::Request::post("https://api.example.com/v1/repos")
            .body(())
            .unwrap();
        assert_eq!(named(b.authorize(&mut post)), Err("operation-not-allowed"));
    }

    #[test]
    fn overrides_userinfo_and_noncanonical_destinations_are_handled() {
        let b = binding();
        for uri in [
            "https://user:pw@api.example.com/v1/repos",
            "https://api.example.com@evil.example.com/v1/repos",
        ] {
            assert_eq!(
                named(b.authorize(&mut request(uri))),
                Err("destination-not-allowed"),
                "{uri}"
            );
        }
        assert_eq!(
            named(b.authorize(&mut request(
                "https://api.example.com/v1/repos?_method=DELETE"
            ))),
            Err("reserved-header")
        );
        for name in [
            "x-http-method-override",
            "x-original-url",
            "x-forwarded-host",
            "forwarded",
            "max-forwards",
        ] {
            let mut req = request("https://api.example.com/v1/repos");
            req.headers_mut().insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_static("x"),
            );
            assert_eq!(
                named(b.authorize(&mut req)),
                Err("reserved-header"),
                "{name}"
            );
        }

        let mut req = request("https://API.Example.com:443/v1/repos");
        req.headers_mut().insert(
            header::HOST,
            HeaderValue::from_static("API.Example.com:443"),
        );
        assert_eq!(named(b.authorize(&mut req)), Ok(()));
        assert_eq!(req.uri(), "https://api.example.com/v1/repos");
        assert_eq!(req.headers()[header::HOST], "api.example.com");
    }

    #[test]
    fn owned_trailers_are_dropped_from_the_request_body() {
        let mut trailers = http::HeaderMap::new();
        trailers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer x"));
        trailers.insert("x-http-method", HeaderValue::from_static("DELETE"));
        trailers.insert("x-checksum", HeaderValue::from_static("abc"));
        let body = http_body_util::StreamBody::new(futures::stream::iter([
            Ok::<_, wasmtime_wasi_http::Error>(Frame::data(Bytes::from_static(b"hi"))),
            Ok(Frame::trailers(trailers)),
        ]))
        .boxed_unsync();
        let (_response, response) = oneshot::channel();
        let guarded = Upload::new(body, Arc::new(binding()), response);
        let state = Arc::clone(&guarded.state);
        let collected = futures::executor::block_on(guarded.collect()).unwrap();
        let trailers = collected.trailers().cloned().unwrap();
        assert_eq!(collected.to_bytes(), "hi");
        assert!(trailers.get(header::AUTHORIZATION).is_none());
        assert!(trailers.get("x-http-method").is_none());
        assert_eq!(trailers["x-checksum"], "abc");
        assert_eq!(state.ended.load(Ordering::Acquire), BODY_TRAILERS);
        assert_eq!(state.bytes.load(Ordering::Acquire), 2);
        assert!(!state.replayable());
    }

    fn guest_body(
        frames: Vec<Result<Frame<Bytes>, wasmtime_wasi_http::Error>>,
    ) -> (Upload, oneshot::Sender<()>) {
        let (response, lifeline) = oneshot::channel();
        let body = http_body_util::StreamBody::new(futures::stream::iter(frames)).boxed_unsync();
        (Upload::new(body, Arc::new(binding()), lifeline), response)
    }

    #[test]
    fn a_guest_body_error_becomes_the_brokers_own() {
        let (upload, _response) = guest_body(vec![
            Ok(Frame::data(Bytes::from_static(b"hi"))),
            Err(wasmtime_wasi_http::Error::DestinationIpProhibited),
        ]);
        let err = futures::executor::block_on(upload.collect()).unwrap_err();
        assert!(
            matches!(&err, HttpError::InternalError(Some(m)) if m == GUEST_BODY_FAILED),
            "{err:?}"
        );
        assert!(matches!(classify(&err), SendError::Indeterminate(_)));
    }

    #[test]
    fn dropping_the_response_fails_the_upload() {
        let (upload, response) = guest_body(vec![Ok(Frame::data(Bytes::from_static(b"hi")))]);
        let state = Arc::clone(&upload.state);
        drop(response);
        let err = futures::executor::block_on(upload.collect()).unwrap_err();
        assert!(matches!(&err, HttpError::InternalError(Some(m)) if m == RESPONSE_DROPPED));
        assert_eq!(state.ended.load(Ordering::Acquire), BODY_OPEN);

        let (upload, _response) = guest_body(vec![Ok(Frame::data(Bytes::from_static(b"hi")))]);
        let state = Arc::clone(&upload.state);
        assert_eq!(
            futures::executor::block_on(upload.collect())
                .unwrap()
                .to_bytes(),
            "hi"
        );
        assert_eq!(state.ended.load(Ordering::Acquire), BODY_CLEAN);
        assert!(!state.replayable(), "two bytes were sent");

        let (upload, _response) = guest_body(vec![]);
        let state = Arc::clone(&upload.state);
        futures::executor::block_on(upload.collect()).unwrap();
        assert!(state.replayable(), "an empty body that ended cleanly");
    }

    #[test]
    fn headers_the_broker_owns_are_refused() {
        let b = binding();
        for (name, value) in [
            ("authorization", "Bearer mine"),
            ("proxy-authorization", "x"),
            ("dpop", "x"),
            ("host", "evil.example.com"),
            ("x-forwarded", "wash_ph_abc"),
            ("x-real-ip", "10.0.0.1"),
            ("true-client-ip", "10.0.0.1"),
            ("client-ip", "10.0.0.1"),
            ("x-client-ip", "10.0.0.1"),
            ("x-cluster-client-ip", "10.0.0.1"),
            ("cf-connecting-ip", "10.0.0.1"),
            ("fastly-client-ip", "10.0.0.1"),
            ("x-originating-ip", "10.0.0.1"),
            ("via", "1.1 proxy"),
        ] {
            let mut req = request("https://api.example.com/v1/repos");
            req.headers_mut().insert(
                HeaderName::from_static(name),
                HeaderValue::from_static(value),
            );
            assert_eq!(
                named(b.authorize(&mut req)),
                Err("reserved-header"),
                "{name}"
            );
        }
        let mut req = request("https://api.example.com/v1/repos");
        req.headers_mut()
            .insert(header::HOST, HeaderValue::from_static("api.example.com"));
        assert_eq!(named(b.authorize(&mut req)), Ok(()));

        let keyed = ClientBinding::new(Presenter::Header(
            HeaderName::from_static("x-api-key"),
            source(b"k"),
        ))
        .to("api.example.com", 443);
        let mut req = request("https://api.example.com/");
        req.headers_mut()
            .insert("x-api-key", HeaderValue::from_static("guess"));
        assert_eq!(named(keyed.authorize(&mut req)), Err("reserved-header"));
    }

    #[test]
    fn presenting_writes_a_sensitive_credential_and_marks_the_request() {
        let mut req = request("https://api.example.com/v1/repos");
        binding().present(&mut req).unwrap();
        let value = &req.headers()[header::AUTHORIZATION];
        assert_eq!(value, "Bearer tok");
        assert!(value.is_sensitive());
        assert!(req.extensions().get::<CarriesSecret>().is_some());
        assert!(req.extensions().get::<BrokeredRequest>().is_some());

        let lapsed = ClientBinding::new(Presenter::Bearer(Arc::new(LeasedSource::default())));
        let mut req = request("https://api.example.com/");
        assert_eq!(
            named(lapsed.present(&mut req).map(drop)),
            Err("credential-unavailable")
        );
    }

    #[test]
    fn the_credential_is_scrubbed_from_the_response_head_and_trailers() {
        let mut req = request("https://api.example.com/v1/repos");
        let scrub = Arc::new(binding().present(&mut req).unwrap());

        let mut headers = http::HeaderMap::new();
        headers.insert("x-full", HeaderValue::from_static("Bearer tok"));
        headers.insert("x-bare", HeaderValue::from_static("id=tok;"));
        headers.insert("x-encoded", HeaderValue::from_static("%74%6F%6B"));
        headers.insert("x-other", HeaderValue::from_static("fine"));
        scrub.revert(&mut headers);
        assert_eq!(headers["x-full"], "[redacted]");
        assert_eq!(headers["x-bare"], "id=[redacted];");
        assert!(!headers.contains_key("x-encoded"));
        assert_eq!(headers["x-other"], "fine");

        let mut trailers = http::HeaderMap::new();
        trailers.insert("x-echo", HeaderValue::from_static("Bearer tok"));
        let frames: Vec<Result<_, HttpError>> = vec![
            Ok(Frame::data(Bytes::from_static(b"tok stays in the body"))),
            Ok(Frame::trailers(trailers)),
        ];
        let (_lifeline, upload) = oneshot::channel();
        let held = Held {
            body: http_body_util::StreamBody::new(futures::stream::iter(frames)).boxed_unsync(),
            scrub,
            _task: None,
            _upload: _lifeline,
        };
        drop(upload);
        let collected = futures::executor::block_on(held.collect()).unwrap();
        assert_eq!(collected.trailers().unwrap()["x-echo"], "[redacted]");
        assert_eq!(collected.to_bytes(), "tok stays in the body");
    }

    #[test]
    fn a_dpop_token_is_presented_with_a_proof_over_the_final_request() {
        use base64::Engine as _;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;

        let key = Arc::new(crate::host::dpop::DpopKey::generate().unwrap());
        let exchange = TokenExchange::new(
            "https://idp.example/token",
            crate::host::token_exchange::Grant::JwtBearer,
            source(b"svid"),
            crate::host::http_client::default_client_tls_config(),
        )
        .unwrap()
        .with_dpop(key)
        .with_test_token(b"bound-token", TokenType::Dpop);
        let binding =
            ClientBinding::new(Presenter::Token(Arc::new(exchange))).to("api.example.com", 443);
        let mut req = http::Request::put("https://api.example.com/v1/items/7?x=1")
            .body(())
            .unwrap();
        binding.present(&mut req).unwrap();
        assert_eq!(req.headers()[header::AUTHORIZATION], "DPoP bound-token");
        let proof = req.headers()[DPOP].to_str().unwrap().to_string();
        let claims: serde_json::Value =
            serde_json::from_slice(&B64.decode(proof.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["htm"], "PUT");
        assert_eq!(claims["htu"], "https://api.example.com/v1/items/7");
        assert_eq!(
            claims["ath"],
            B64.encode(aws_lc_rs::digest::digest(
                &aws_lc_rs::digest::SHA256,
                b"bound-token"
            ))
        );
        assert!(binding.dpop_key().is_some());
    }

    #[test]
    fn a_failure_before_sending_is_not_reported_as_indeterminate() {
        use wasmtime_wasi_http::Error as E;
        assert!(matches!(
            classify(&E::HttpRequestDenied),
            SendError::Denied(DeniedReason::NotEntitled)
        ));
        assert!(matches!(
            classify(&E::DestinationIpProhibited),
            SendError::Denied(DeniedReason::ResolvedAddressNotAllowed)
        ));
        assert!(matches!(
            classify(&E::ConnectionRefused),
            SendError::Transport(_)
        ));
        assert!(matches!(
            classify(&E::HttpResponseTimeout),
            SendError::Indeterminate(_)
        ));
    }
}

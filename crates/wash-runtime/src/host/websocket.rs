//! WebSocket bridge for components that export
//! `wasmcloud:websocket/handler@0.1`.
//!
//! Flow:
//!   1. Validate `Sec-WebSocket-Key`, compute `Sec-WebSocket-Accept`.
//!   2. Capture hyper's pending upgrade future *before* returning the
//!      response, so we can wait on the actual TCP stream once hyper
//!      has flushed the 101.
//!   3. Return `101 Switching Protocols`; hyper completes the upgrade.
//!   4. In a detached task: await the upgrade, wrap the post-upgrade
//!      stream in `tokio_tungstenite::WebSocketStream`, split it into
//!      read / write halves, and bridge each half to a host-owned
//!      `stream<frame>` via wasmtime's `StreamProducer` /
//!      `StreamConsumer` traits.
//!   5. Invoke the component's `handle` export. The component returns
//!      a `StreamReader<Frame>` for the outgoing direction; we `.pipe`
//!      it into the WS write half. Closing happens when either the
//!      peer disconnects (`incoming` ends) or the component drops its
//!      outgoing writer.

use std::pin::Pin;
use std::task::{Context, Poll};

use crate::engine::ctx::SharedCtx;
use crate::engine::workload::ResolvedWorkload;
use crate::observability::FuelConsumptionMeter;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::channel::mpsc;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use sha1::{Digest, Sha1};
use tokio::sync::oneshot;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, frame::coding::CloseCode};
use wasmtime::Store;
use wasmtime::component::{
    Destination, InstancePre, Source, StreamConsumer, StreamProducer, StreamReader, StreamResult,
    VecBuffer,
};
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "websocket",
        exports: { default: async },
    });
}

use bindings::WebsocketPre;
use bindings::exports::wasmcloud::websocket::handler::{
    Frame as GuestFrame, UpgradeRequest as GuestUpgradeRequest,
};
use bindings::wasmcloud::websocket::types::CloseInfo as GuestCloseInfo;

/// RFC 6455 §1.3 magic GUID used to derive `Sec-WebSocket-Accept` from
/// the client's `Sec-WebSocket-Key`.
const WS_MAGIC_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const WS_WRITE_QUEUE_CAPACITY: usize = 1;

/// Bridge an HTTP `Upgrade: websocket` request into a component that
/// exports `wasmcloud:websocket/handler@0.1`.
///
/// The caller must have already invoked
/// `pre_instantiate_linked_components_for_component` on the store so
/// any peer components the WS handler links against are available.
pub async fn handle_websocket_request(
    workload_handle: ResolvedWorkload,
    store: Store<SharedCtx>,
    pre: InstancePre<SharedCtx>,
    mut req: hyper::Request<hyper::body::Incoming>,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let _ = &fuel_meter;

    let accept = match compute_accept_header(&req) {
        Some(v) => v,
        None => {
            tracing::warn!("ws upgrade missing or invalid Sec-WebSocket-Key");
            return Ok(error_resp(400, "invalid websocket key"));
        }
    };

    let ws_pre = match WebsocketPre::new(pre) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(err = %e, "failed to build WebsocketPre");
            return Ok(error_resp(500, "ws handler bindgen failed"));
        }
    };

    let guest_req = build_guest_request(&req);

    // Capture the upgrade future before returning the response. Hyper
    // resolves this once the 101 has been flushed.
    let upgrade_fut = hyper::upgrade::on(&mut req);

    tokio::spawn(run_ws_session(
        workload_handle,
        store,
        ws_pre,
        guest_req,
        upgrade_fut,
    ));

    Ok(switching_protocols_response(accept))
}

fn switching_protocols_response(accept: String) -> hyper::Response<HyperOutgoingBody> {
    let body = http_body_util::Empty::<bytes::Bytes>::new()
        .map_err(|never| match never {})
        .boxed_unsync();
    hyper::Response::builder()
        .status(hyper::StatusCode::SWITCHING_PROTOCOLS)
        .header(hyper::header::CONNECTION, "Upgrade")
        .header(hyper::header::UPGRADE, "websocket")
        .header(hyper::header::SEC_WEBSOCKET_ACCEPT, accept)
        .body(HyperOutgoingBody::new(body))
        .expect("101 response with static headers is well-formed")
}

fn error_resp(status: u16, msg: &'static str) -> hyper::Response<HyperOutgoingBody> {
    let body = http_body_util::Full::new(bytes::Bytes::from_static(msg.as_bytes()))
        .map_err(|never: std::convert::Infallible| match never {})
        .boxed_unsync();
    hyper::Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain")
        .body(HyperOutgoingBody::new(body))
        .expect("error response is well-formed")
}

fn compute_accept_header(req: &hyper::Request<hyper::body::Incoming>) -> Option<String> {
    let key = req
        .headers()
        .get(hyper::header::SEC_WEBSOCKET_KEY)?
        .to_str()
        .ok()?
        .trim();
    if key.is_empty() {
        return None;
    }
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_MAGIC_GUID);
    Some(STANDARD.encode(hasher.finalize()))
}

fn build_guest_request(req: &hyper::Request<hyper::body::Incoming>) -> GuestUpgradeRequest {
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let headers = req
        .headers()
        .iter()
        .map(|(name, value)| (name.as_str().to_string(), value.as_bytes().to_vec()))
        .collect();
    let subprotocols = req
        .headers()
        .get_all(hyper::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(',').map(|p| p.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    GuestUpgradeRequest {
        path,
        query,
        headers,
        subprotocols,
    }
}

async fn run_ws_session(
    workload_handle: ResolvedWorkload,
    mut store: Store<SharedCtx>,
    ws_pre: WebsocketPre<SharedCtx>,
    guest_req: GuestUpgradeRequest,
    upgrade_fut: hyper::upgrade::OnUpgrade,
) {
    let store_id = store.data().active_ctx.store_id.clone();
    let upgraded = match upgrade_fut.await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!(err = %e, "ws upgrade future failed");
            return;
        }
    };

    let ws = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
    let (ws_write, ws_read) = ws.split();

    let instance = match ws_pre.instantiate_async(&mut store).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!(err = %e, "failed to instantiate ws component");
            return;
        }
    };

    let (close_tx, close_rx) = oneshot::channel::<()>();
    let producer = WsReadProducer::new(ws_read);
    let (write_tx, write_rx) = mpsc::channel(WS_WRITE_QUEUE_CAPACITY);
    spawn_ws_write_forwarder(ws_write, write_rx, close_tx);
    let consumer = WsWriteConsumer::new(write_tx);

    let result = store
        .run_concurrent(async move |accessor| {
            let incoming = accessor.with(|mut s| StreamReader::new(&mut s, producer))?;
            let call = instance
                .wasmcloud_websocket_handler()
                .call_handle(accessor, guest_req, incoming)
                .await;
            match call {
                Ok(Ok(outgoing)) => {
                    let pipe_res = accessor.with(|mut s| outgoing.pipe(&mut s, consumer));
                    if let Err(e) = pipe_res {
                        tracing::error!(err = ?e, "ws pipe to write half failed");
                    }
                    let _ = close_rx.await;
                    Ok::<_, anyhow::Error>(())
                }
                Ok(Err(msg)) => {
                    tracing::warn!(reason = %msg, "ws handler returned Err; closing 1011");
                    drop(close_rx);
                    Ok(())
                }
                Err(e) => {
                    tracing::error!(err = ?e, "ws handler trapped");
                    drop(close_rx);
                    Ok(())
                }
            }
        })
        .await;

    if let Err(e) = result {
        tracing::error!(err = ?e, "ws run_concurrent failed");
    }

    // Mirror the p3 HTTP path: drop per-store exporter instances so we
    // don't leak linked-component state across sessions.
    workload_handle.clear_exporter_instances_for_store(&store_id);
}

// ---------------------------------------------------------------------
// Producer: WS read half → guest `incoming: stream<frame>`
// ---------------------------------------------------------------------

struct WsReadProducer {
    ws_read: SplitStream<WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>>,
    /// Buffered frame if `poll_produce` was woken with a zero-capacity
    /// destination. Held until the next non-zero call.
    buffered: Option<GuestFrame>,
}

impl WsReadProducer {
    fn new(ws_read: SplitStream<WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>>) -> Self {
        Self {
            ws_read,
            buffered: None,
        }
    }
}

impl StreamProducer<SharedCtx> for WsReadProducer {
    type Item = GuestFrame;
    type Buffer = VecBuffer<GuestFrame>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: wasmtime::StoreContextMut<SharedCtx>,
        mut destination: Destination<'a, Self::Item, Self::Buffer>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let cap = destination.remaining(store).unwrap_or(usize::MAX);

        if let Some(frame) = self.buffered.take() {
            if cap == 0 {
                self.buffered = Some(frame);
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            destination.set_buffer(VecBuffer::from(vec![frame]));
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        loop {
            match self.ws_read.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(Ok(StreamResult::Dropped)),
                Poll::Ready(Some(Err(e))) => {
                    tracing::warn!(err = %e, "ws read error; treating as close");
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Poll::Ready(Some(Ok(msg))) => {
                    let Some(frame) = ws_message_to_frame(msg) else {
                        // Ping / Pong / Frame are absorbed by tungstenite
                        // or by us; loop to fetch the next user frame.
                        continue;
                    };
                    if cap == 0 {
                        self.buffered = Some(frame);
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }
                    destination.set_buffer(VecBuffer::from(vec![frame]));
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
            }
        }
    }
}

fn ws_message_to_frame(msg: WsMessage) -> Option<GuestFrame> {
    match msg {
        WsMessage::Text(t) => Some(GuestFrame::Text(t.to_string())),
        WsMessage::Binary(b) => Some(GuestFrame::Binary(b.to_vec())),
        WsMessage::Close(Some(cf)) => Some(GuestFrame::Close(GuestCloseInfo {
            code: u16::from(cf.code),
            reason: cf.reason.to_string(),
        })),
        WsMessage::Close(None) => Some(GuestFrame::Close(GuestCloseInfo {
            code: 1005,
            reason: String::new(),
        })),
        WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_) => None,
    }
}

// ---------------------------------------------------------------------
// Consumer: guest `outgoing: stream<frame>` → WS write half
// ---------------------------------------------------------------------

fn spawn_ws_write_forwarder(
    mut ws_write: SplitSink<WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>, WsMessage>,
    mut write_rx: mpsc::Receiver<WsMessage>,
    close_tx: oneshot::Sender<()>,
) {
    tokio::spawn(async move {
        while let Some(msg) = write_rx.next().await {
            let is_close = matches!(msg, WsMessage::Close(_));
            if let Err(e) = ws_write.send(msg).await {
                tracing::warn!(err = %e, "ws write send failed");
                break;
            }
            if is_close {
                break;
            }
        }

        if let Err(e) = ws_write.close().await {
            tracing::debug!(err = %e, "ws write close failed");
        }
        let _ = close_tx.send(());
    });
}

struct WsWriteConsumer {
    write_tx: mpsc::Sender<WsMessage>,
    closed: bool,
}

impl WsWriteConsumer {
    fn new(write_tx: mpsc::Sender<WsMessage>) -> Self {
        Self {
            write_tx,
            closed: false,
        }
    }
}

impl StreamConsumer<SharedCtx> for WsWriteConsumer {
    type Item = GuestFrame;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: wasmtime::StoreContextMut<SharedCtx>,
        mut src: Source<Self::Item>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        use wasmtime::AsContextMut;

        if self.closed {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        // Pull one frame at a time, but only after the forwarder queue
        // has capacity. The forwarder owns the websocket sink and uses
        // `send().await`, which drives tungstenite's flush to completion.
        match self.write_tx.poll_ready_unpin(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => {
                tracing::warn!(err = %e, "ws write forwarder is closed");
                self.closed = true;
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            Poll::Ready(Ok(())) => {}
        }

        let mut buf: Vec<GuestFrame> = Vec::with_capacity(1);
        src.read(store.as_context_mut(), &mut buf)?;
        if buf.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let frame = buf.remove(0);
        let (ws_msg, is_close) = guest_frame_to_message(frame);

        if let Err(e) = self.write_tx.start_send_unpin(ws_msg) {
            tracing::warn!(err = %e, "ws write enqueue failed");
            self.closed = true;
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        if is_close {
            self.closed = true;
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        Poll::Ready(Ok(StreamResult::Completed))
    }
}

fn guest_frame_to_message(frame: GuestFrame) -> (WsMessage, bool) {
    match frame {
        GuestFrame::Text(s) => (WsMessage::Text(s.into()), false),
        GuestFrame::Binary(b) => (WsMessage::Binary(b.into()), false),
        GuestFrame::Close(info) => {
            let cf = CloseFrame {
                code: CloseCode::from(info.code),
                reason: info.reason.into(),
            };
            (WsMessage::Close(Some(cf)), true)
        }
    }
}

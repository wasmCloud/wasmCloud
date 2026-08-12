//! Drain a guest's `stream<u8>` into memory, bounded.
//!
//! Several async host impls take a `stream<u8>` from a guest but hand a
//! *complete* byte payload to their backend — a message body going to core
//! NATS, an object body going to a blobstore `write_data`. Each one has to pipe
//! the reader into a consumer, wait for the stream to end, and turn the result
//! into its own WIT error type. That mechanism is identical everywhere and
//! lives here; only the limit and the error mapping differ per caller.
//!
//! A backend that can forward a stream incrementally should not use this — it
//! exists for the ones that cannot.

use wasmtime::component::{Accessor, HasData, Source, StreamConsumer, StreamReader, StreamResult};

/// Why a collection did not produce a complete body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CollectError {
    /// The stream tried to deliver more than the caller's limit. Reported
    /// without buffering the excess, so an unbounded guest cannot balloon host
    /// memory before the backend's own size limit gets a chance to reject it.
    LimitExceeded { limit: usize },
    /// The collector went away without reporting. Unreachable while `Drop` is
    /// the reporting path, and surfaced rather than silently read as an empty
    /// body if that ever stops being true.
    Abandoned,
}

impl std::fmt::Display for CollectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollectError::LimitExceeded { limit } => {
                write!(
                    f,
                    "stream exceeded the host collection limit of {limit} bytes"
                )
            }
            CollectError::Abandoned => {
                f.write_str("stream collector was dropped without reporting")
            }
        }
    }
}

/// Collect every byte `stream` delivers, up to `limit`.
///
/// Pass [`usize::MAX`] for an unbounded collection.
///
/// A stream the guest abandons part-way is indistinguishable from one it ended
/// deliberately: both yield whatever bytes arrived. There is no separate
/// end-of-stream signal to check, so a caller cannot tell a truncated body from
/// a complete one.
pub(crate) async fn collect_stream<T, D>(
    accessor: &Accessor<T, D>,
    stream: StreamReader<u8>,
    limit: usize,
) -> wasmtime::Result<Result<Vec<u8>, CollectError>>
where
    T: 'static,
    D: HasData,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Vec<u8>, CollectError>>();
    accessor.with(|mut a| {
        stream.pipe(
            &mut a,
            CollectConsumer {
                buf: Vec::new(),
                limit,
                done: Some(tx),
            },
        )
    })?;
    Ok(rx.await.unwrap_or(Err(CollectError::Abandoned)))
}

/// A [`StreamConsumer`] that accumulates every byte written and hands the
/// buffer back once the stream ends. The runtime drops the consumer at
/// end-of-stream, which fires [`Drop`] and delivers the bytes over `done`.
struct CollectConsumer {
    buf: Vec<u8>,
    limit: usize,
    done: Option<tokio::sync::oneshot::Sender<Result<Vec<u8>, CollectError>>>,
}

impl Drop for CollectConsumer {
    fn drop(&mut self) {
        if let Some(tx) = self.done.take() {
            let _ = tx.send(Ok(std::mem::take(&mut self.buf)));
        }
    }
}

impl<D> StreamConsumer<D> for CollectConsumer {
    type Item = u8;

    fn poll_consume(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        store: wasmtime::StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> std::task::Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        let mut src = src.as_direct(store);
        let bytes = src.remaining();
        if bytes.is_empty() {
            // No items offered (count == 0). This is an unbounded in-memory
            // sink, so it is always ready to accept; the actual end-of-stream
            // is observed via `Drop`.
            return std::task::Poll::Ready(Ok(if finish {
                StreamResult::Cancelled
            } else {
                StreamResult::Completed
            }));
        }
        let n = bytes.len();
        if this.buf.len().saturating_add(n) > this.limit {
            // Refuse the excess instead of buffering it; delivering the error
            // here (rather than via `Drop`) is what lets the caller see the
            // limit failure instead of a truncated body.
            //
            // `Dropped` — not `Cancelled` — is the disposition for refusing
            // items: it means "this consumer will accept no more", which is
            // exactly the case, and it is what wasmtime documents for an error
            // reported by other means (here, `done`). `Cancelled` is reserved
            // for wrapping up early under `finish`, and returning it without
            // taking an item while `finish` is false traps the caller.
            if let Some(tx) = this.done.take() {
                let _ = tx.send(Err(CollectError::LimitExceeded { limit: this.limit }));
            }
            return std::task::Poll::Ready(Ok(StreamResult::Dropped));
        }
        this.buf.extend_from_slice(bytes);
        src.mark_read(n);
        std::task::Poll::Ready(Ok(StreamResult::Completed))
    }
}

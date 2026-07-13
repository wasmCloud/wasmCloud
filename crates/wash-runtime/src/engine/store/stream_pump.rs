//! Live, **no-buffering** stream pump across the store boundary, generic over
//! the element type.
//!
//! A `stream<T>` is relayed through a **bounded** `futures::channel::mpsc`
//! channel of `Vec<T>` chunks:
//!
//! - Source store: [`ChannelConsumer`] is `pipe`d the source `StreamReader<T>`;
//!   each `poll_consume` waits for channel room (`Sender::poll_ready` —
//!   backpressure) then forwards the available items.
//! - Destination store: [`ChannelProducer`] backs a fresh `StreamReader<T>`;
//!   each `poll_produce` pulls the next chunk from the channel.
//!
//! The bounded channel caps in-flight data and back-pressures the source, so the
//! stream is never fully buffered. The element type is chosen by the caller via
//! the func signature (see [`super::relocate::stream_factory`]).

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::StreamExt as _;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Destination, FutureConsumer, Lift, Lower, Source, StreamConsumer, StreamProducer, StreamResult,
    VecBuffer,
};

/// Default in-flight chunk capacity of the cross-store channel (backpressure).
pub const DEFAULT_CAPACITY: usize = 8;

/// Destination side: yields chunks pulled from the channel.
pub struct ChannelProducer<T> {
    rx: futures::channel::mpsc::Receiver<Vec<T>>,
}

impl<T, D> StreamProducer<D> for ChannelProducer<T>
where
    T: Lower + Send + Sync + 'static,
{
    type Item = T;
    type Buffer = VecBuffer<T>;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        match self.get_mut().rx.poll_next_unpin(cx) {
            Poll::Ready(Some(chunk)) => {
                dst.set_buffer(VecBuffer::from(chunk));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(None) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Source side: forwards source items into the channel, back-pressuring when
/// full. Fires `done` once the source ends, so a caller can know the pump has
/// drained (e.g. an ephemeral store that must outlive a result stream).
pub struct ChannelConsumer<T> {
    tx: futures::channel::mpsc::Sender<Vec<T>>,
    done: Option<futures::channel::oneshot::Sender<()>>,
}

impl<T> ChannelConsumer<T> {
    fn finish(&mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
    }
}

impl<T, D: 'static> StreamConsumer<D> for ChannelConsumer<T>
where
    T: Lift + Send + Sync + 'static,
{
    type Item = T;

    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<D>,
        mut source: Source<'_, Self::Item>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        match this.tx.poll_ready(cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(_)) => {
                this.finish();
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            Poll::Pending => return Poll::Pending,
        }
        let available = source.remaining(&mut store);
        if available == 0 {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let mut buf: Vec<T> = Vec::with_capacity(available);
        source.read(&mut store, &mut buf)?;
        if buf.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let _ = this.tx.start_send(buf);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl<T> Drop for ChannelConsumer<T> {
    fn drop(&mut self) {
        self.finish();
    }
}

/// A pump's completion signal: resolves once the source stream has fully drained
/// into the channel.
pub type Done = futures::channel::oneshot::Receiver<()>;

/// Create a connected consumer/producer pair over a bounded channel, plus a
/// [`Done`] that fires when the source drains.
pub fn channel<T>(capacity: usize) -> (ChannelConsumer<T>, ChannelProducer<T>, Done) {
    let (tx, rx) = futures::channel::mpsc::channel(capacity);
    let (done_tx, done_rx) = futures::channel::oneshot::channel();
    (
        ChannelConsumer {
            tx,
            done: Some(done_tx),
        },
        ChannelProducer { rx },
        done_rx,
    )
}

/// Source side of a `future<T>` pump: forwards the source future's single value
/// over a oneshot, then fires `done`. The destination future is built from the
/// paired receiver (an `async move { rx.await }` is a [`wasmtime::component::FutureProducer`]
/// via the blanket impl), so no producer struct is needed.
pub struct FutureSink<T> {
    tx: Option<futures::channel::oneshot::Sender<T>>,
    done: Option<futures::channel::oneshot::Sender<()>>,
}

impl<T> FutureSink<T> {
    fn finish(&mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
    }
}

impl<T, D: 'static> FutureConsumer<D> for FutureSink<T>
where
    T: Lift + Send + Sync + 'static,
{
    type Item = T;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<D>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<()>> {
        let this = self.get_mut();
        if source.remaining(&mut store) == 0 {
            // `finish` means the read was cancelled before a value arrived.
            if finish {
                this.finish();
                return Poll::Ready(Ok(()));
            }
            // A future write always presents its single value when the consumer
            // is polled (`remaining() >= 1`), so this branch is unreachable.
            // Returning a bare `Poll::Pending` here would register no waker and
            // hang the guest's write; fail loudly instead if that ever changes.
            this.finish();
            return Poll::Ready(Err(wasmtime::format_err!(
                "future bridge: consumer polled with no value available"
            )));
        }
        let mut buf: Vec<T> = Vec::with_capacity(1);
        source.read(&mut store, &mut buf)?;
        if let Some(v) = buf.into_iter().next()
            && let Some(tx) = this.tx.take()
        {
            let _ = tx.send(v);
        }
        this.finish();
        Poll::Ready(Ok(()))
    }
}

impl<T> Drop for FutureSink<T> {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Create a [`FutureSink`] (source side) paired with the receiver the
/// destination future awaits, plus a [`Done`] that fires once the value has been
/// forwarded (or the sink is dropped).
pub fn future_channel<T>() -> (FutureSink<T>, futures::channel::oneshot::Receiver<T>, Done) {
    let (tx, rx) = futures::channel::oneshot::channel();
    let (done_tx, done_rx) = futures::channel::oneshot::channel();
    (
        FutureSink {
            tx: Some(tx),
            done: Some(done_tx),
        },
        rx,
        done_rx,
    )
}

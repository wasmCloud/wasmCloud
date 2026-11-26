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

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use futures::{FutureExt as _, StreamExt as _};
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Destination, FutureConsumer, Lift, Lower, Source, StreamConsumer, StreamProducer, StreamResult,
    VecBuffer,
};

/// Default in-flight chunk capacity of the cross-store channel (backpressure).
pub const DEFAULT_CAPACITY: usize = 8;

/// How a pump's source ended, shared by both of its sides.
#[derive(Default)]
struct Ending {
    /// The source ended: its writer closed, or the destination went away.
    finished: AtomicBool,
    /// The source's store went away before the source ended.
    cut: AtomicBool,
    /// The destination dropped its end, so no reader waits on the rest.
    reader_gone: AtomicBool,
}

/// Destination side: yields chunks pulled from the channel.
pub struct ChannelProducer<T> {
    rx: futures::channel::mpsc::Receiver<Vec<T>>,
    ending: Arc<Ending>,
}

impl<T> Drop for ChannelProducer<T> {
    fn drop(&mut self) {
        self.ending.reader_gone.store(true, Ordering::SeqCst);
    }
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
        let this = self.get_mut();
        match this.rx.poll_next_unpin(cx) {
            Poll::Ready(Some(chunk)) => {
                dst.set_buffer(VecBuffer::from(chunk));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            // A stream can only end or fail; a cut one must not read as finished.
            Poll::Ready(None) if this.ending.cut.load(Ordering::SeqCst) => Poll::Ready(Err(
                wasmtime::format_err!("relocated stream was cut off before its source ended"),
            )),
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
    ending: Arc<Ending>,
}

impl<T> ChannelConsumer<T> {
    fn finish(&mut self) {
        self.ending.finished.store(true, Ordering::SeqCst);
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
        // `poll_ready` reserved a slot above, so this only fails if the receiver
        // disconnected in between; report the drop instead of losing the chunk
        // silently.
        if this.tx.start_send(buf).is_err() {
            this.finish();
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
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
pub struct Done {
    drained: futures::channel::oneshot::Receiver<()>,
    ending: Arc<Ending>,
}

impl Future for Done {
    type Output = Result<(), futures::channel::oneshot::Canceled>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().drained.poll_unpin(cx)
    }
}

/// One pump's ending, as its source's store sees it.
struct Cut(Arc<Ending>);

impl Cut {
    /// The source has not ended and a reader still waits on it.
    fn open(&self) -> bool {
        !self.0.finished.load(Ordering::SeqCst) && !self.0.reader_gone.load(Ordering::SeqCst)
    }

    /// Mark the stream cut unless its source already ended; returns whether a
    /// reader will see the cut.
    fn mark(&self) -> bool {
        if self.0.finished.load(Ordering::SeqCst) {
            return false;
        }
        self.0.cut.store(true, Ordering::SeqCst);
        !self.0.reader_gone.load(Ordering::SeqCst)
    }
}

/// The endings of one call's relocated results, decided before the source's
/// store drops. Dropped undecided, it cuts the open ones, so a task that
/// unwinds still fails their readers.
#[derive(Default)]
pub struct Cuts(Vec<Cut>);

impl Cuts {
    pub fn watch(&mut self, done: &Done) {
        self.0.push(Cut(Arc::clone(&done.ending)));
    }

    /// End the open results with an error at their readers; returns how many
    /// readers will see it. Call it while nothing runs in the source's store.
    pub fn cut(&mut self) -> usize {
        std::mem::take(&mut self.0)
            .iter()
            .filter(|c| c.mark())
            .count()
    }

    /// Let every result end as its source's store leaves it; returns how many
    /// a reader still waits on.
    pub fn release(&mut self) -> usize {
        std::mem::take(&mut self.0)
            .iter()
            .filter(|c| c.open())
            .count()
    }
}

impl Drop for Cuts {
    fn drop(&mut self) {
        self.cut();
    }
}

/// Create a connected consumer/producer pair over a bounded channel, plus a
/// [`Done`] that fires when the source drains.
pub fn channel<T>(capacity: usize) -> (ChannelConsumer<T>, ChannelProducer<T>, Done) {
    let (tx, rx) = futures::channel::mpsc::channel(capacity);
    let (done_tx, done_rx) = futures::channel::oneshot::channel();
    let ending = Arc::new(Ending::default());
    (
        ChannelConsumer {
            tx,
            done: Some(done_tx),
            ending: Arc::clone(&ending),
        },
        ChannelProducer {
            rx,
            ending: Arc::clone(&ending),
        },
        Done {
            drained: done_rx,
            ending,
        },
    )
}

/// Source side of a `future<T>` pump: forwards the source future's single value
/// over a oneshot, then fires `done`. The destination future is built from the
/// paired receiver (an `async move { rx.await }` is a [`wasmtime::component::FutureProducer`]
/// via the blanket impl), so no producer struct is needed.
pub struct FutureSink<T> {
    tx: Option<futures::channel::oneshot::Sender<T>>,
    done: Option<futures::channel::oneshot::Sender<()>>,
    ending: Arc<Ending>,
}

impl<T> FutureSink<T> {
    fn finish(&mut self) {
        self.ending.finished.store(true, Ordering::SeqCst);
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

/// Destination side of a `future<T>` pump: the value, or `Canceled` if the
/// source went before one.
pub struct FutureReceiver<T> {
    rx: futures::channel::oneshot::Receiver<T>,
    ending: Arc<Ending>,
}

impl<T> Future for FutureReceiver<T> {
    type Output = Result<T, futures::channel::oneshot::Canceled>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().rx.poll_unpin(cx)
    }
}

impl<T> Drop for FutureReceiver<T> {
    fn drop(&mut self) {
        self.ending.reader_gone.store(true, Ordering::SeqCst);
    }
}

/// Create a [`FutureSink`] (source side) paired with the receiver the
/// destination future awaits, plus a [`Done`] that fires once the value has been
/// forwarded (or the sink is dropped).
pub fn future_channel<T>() -> (FutureSink<T>, FutureReceiver<T>, Done) {
    let (tx, rx) = futures::channel::oneshot::channel();
    let (done_tx, done_rx) = futures::channel::oneshot::channel();
    // A future's destination already fails when its source goes before a
    // value; the ending only lets a cut count it.
    let ending = Arc::new(Ending::default());
    (
        FutureSink {
            tx: Some(tx),
            done: Some(done_tx),
            ending: Arc::clone(&ending),
        },
        FutureReceiver {
            rx,
            ending: Arc::clone(&ending),
        },
        Done {
            drained: done_rx,
            ending,
        },
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn is_cut(done: &Done) -> bool {
        done.ending.cut.load(Ordering::SeqCst)
    }

    /// A cut applies only to a stream whose source has not ended, so a stream
    /// that finished before its store went away still reads as finished.
    #[test]
    fn a_cut_spares_a_stream_whose_source_already_ended() {
        let (mut finished, _finished_reader, finished_done) = channel::<u8>(1);
        finished.finish();
        let (_open, _open_reader, open_done) = channel::<u8>(1);
        let (mut resolved, _resolved_reader, resolved_done) = future_channel::<u8>();
        resolved.finish();
        let mut cuts = Cuts::default();
        for done in [&finished_done, &open_done, &resolved_done] {
            cuts.watch(done);
        }
        assert_eq!(cuts.cut(), 1);
        assert!(!is_cut(&finished_done));
        assert!(is_cut(&open_done));
        assert!(!is_cut(&resolved_done));
    }

    /// A task that unwinds before it decides still fails the readers of its
    /// open results.
    #[test]
    fn dropping_the_cuts_cuts_what_is_still_open() {
        let (_source, _reader, done) = channel::<u8>(1);
        let mut cuts = Cuts::default();
        cuts.watch(&done);
        drop(cuts);
        assert!(is_cut(&done));
    }

    /// A release cuts nothing, also when dropped later, and counts only results
    /// that a reader still waits on.
    #[test]
    fn a_release_cuts_nothing_and_counts_only_held_open_results() {
        let (_held_source, _held_reader, held) = channel::<u8>(1);
        let (_dropped_source, dropped_reader, dropped) = channel::<u8>(1);
        drop(dropped_reader);
        let (_future_source, future_reader, future) = future_channel::<u8>();
        drop(future_reader);
        let mut cuts = Cuts::default();
        for done in [&held, &dropped, &future] {
            cuts.watch(done);
        }
        assert_eq!(cuts.release(), 1);
        drop(cuts);
        assert!(!is_cut(&held) && !is_cut(&dropped) && !is_cut(&future));
    }
}

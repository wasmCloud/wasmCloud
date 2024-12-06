//! wasmCloud I/O functionality

use core::pin::Pin;
use core::task::{Context, Poll};

use futures::{Stream, StreamExt as _};

/// Incoming value [`Stream`] wrapper, which buffers the chunks and flattens them
pub struct BufferedIncomingStream<T> {
    stream: Pin<Box<dyn Stream<Item = Vec<T>> + Send>>,
    buffer: Vec<T>,
}

impl<T> BufferedIncomingStream<T> {
    /// Create a new [`BufferedIncomingStream`]
    #[must_use]
    pub fn new(stream: Pin<Box<dyn Stream<Item = Vec<T>> + Send>>) -> Self {
        Self {
            stream,
            buffer: Vec::default(),
        }
    }
}

impl<T> Stream for BufferedIncomingStream<T>
where
    T: Unpin,
{
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.buffer.is_empty() {
            match self.stream.poll_next_unpin(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Ready(Some(mut values)) => match values.len() {
                    0 => Poll::Ready(None),
                    1 => {
                        let item = values.pop().expect("element missing");
                        Poll::Ready(Some(item))
                    }
                    _ => {
                        self.buffer = values.split_off(1);
                        let item = values.pop().expect("element missing");
                        assert!(values.is_empty());
                        Poll::Ready(Some(item))
                    }
                },
            }
        } else {
            let tail = self.buffer.split_off(1);
            let item = self.buffer.pop().expect("element missing");
            assert!(self.buffer.is_empty());
            self.buffer = tail;
            Poll::Ready(Some(item))
        }
    }
}

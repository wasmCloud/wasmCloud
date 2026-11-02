//! Replies held outside a long-lived store's `run_concurrent`.
//!
//! A guest trap ends [`wasmtime::Store::run_concurrent`] with every task still
//! on the store, never to run again, along with any reply sender moved into
//! one. Whoever was waiting learns only that the call went away, once the store
//! drops. A driver that serves many calls on one store lends each call its
//! reply from a [`LentReplies`] it keeps outside the store, and answers what is
//! still lent with the trap.

use std::any::Any;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::oneshot;

/// A reply still lent, of whatever type its call answers with.
trait Lent: Send {
    /// The `Option<oneshot::Sender<R>>` this holds, for its call to take.
    fn reply(&mut self) -> &mut dyn Any;

    /// Answer the reply with the fault that ended its store.
    fn fault(self: Box<Self>, err: &wasmtime::Error);
}

struct Pending<R, F> {
    reply: Option<oneshot::Sender<R>>,
    on_fault: F,
}

impl<R, F> Lent for Pending<R, F>
where
    R: Send + 'static,
    F: FnOnce(&wasmtime::Error) -> R + Send,
{
    fn reply(&mut self) -> &mut dyn Any {
        &mut self.reply
    }

    fn fault(self: Box<Self>, err: &wasmtime::Error) {
        if let Some(reply) = self.reply {
            let _ = reply.send((self.on_fault)(err));
        }
    }
}

/// The replies of the calls in flight on one store.
#[derive(Default)]
pub(crate) struct LentReplies {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next: u64,
    lent: BTreeMap<u64, Box<dyn Lent>>,
}

impl LentReplies {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hold `reply` here until its call answers it, or the store faults and
    /// `on_fault` answers it instead. Lent before the call is spawned, since
    /// the store can fault before the call first runs.
    pub(crate) fn lend<R: Send + 'static>(
        self: &Arc<Self>,
        reply: oneshot::Sender<R>,
        on_fault: impl FnOnce(&wasmtime::Error) -> R + Send + 'static,
    ) -> LentReply<R> {
        let mut state = self.lock();
        let key = state.next;
        state.next += 1;
        let pending = Pending {
            reply: Some(reply),
            on_fault,
        };
        state.lent.insert(key, Box::new(pending));
        LentReply {
            replies: Arc::clone(self),
            key,
            reply: PhantomData,
        }
    }

    /// Answer every reply still lent, for a store that faulted with `err`.
    /// Called before the store drops, while its tasks still hold their
    /// [`LentReply`]s.
    pub(crate) fn fault(&self, err: &wasmtime::Error) {
        let lent = std::mem::take(&mut self.lock().lent);
        for pending in lent.into_values() {
            pending.fault(err);
        }
    }
}

/// One call's reply, lent from its store's [`LentReplies`]. Dropping it
/// unanswered drops the reply, as dropping its sender would.
pub(crate) struct LentReply<R> {
    replies: Arc<LentReplies>,
    key: u64,
    reply: PhantomData<fn(R)>,
}

impl<R: 'static> LentReply<R> {
    /// Answer the call, unless its store's fault already has.
    pub(crate) fn send(self, value: R) {
        let lent = self.replies.lock().lent.remove(&self.key);
        let reply = lent.and_then(|mut lent| {
            lent.reply()
                .downcast_mut::<Option<oneshot::Sender<R>>>()?
                .take()
        });
        if let Some(reply) = reply {
            let _ = reply.send(value);
        }
    }
}

impl<R> Drop for LentReply<R> {
    fn drop(&mut self) {
        let lent = self.replies.lock().lent.remove(&self.key);
        drop(lent);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn a_fault_answers_only_the_replies_still_lent() {
        let replies = Arc::new(LentReplies::default());
        let (answered_tx, mut answered) = oneshot::channel();
        let (dropped_tx, mut dropped) = oneshot::channel::<&str>();
        let (held_tx, mut held) = oneshot::channel();
        let (other_tx, mut other) = oneshot::channel();
        replies.lend(answered_tx, |_| "faulted").send("returned");
        drop(replies.lend(dropped_tx, |_| "faulted"));
        let _held = replies.lend(held_tx, |err| format!("faulted: {err}"));
        let _other = replies.lend(other_tx, |_| 7);
        assert_eq!(
            dropped.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        );

        replies.fault(&wasmtime::format_err!("trap"));
        assert_eq!(answered.try_recv(), Ok("returned"));
        assert_eq!(held.try_recv().as_deref(), Ok("faulted: trap"));
        assert_eq!(other.try_recv(), Ok(7));
    }
}

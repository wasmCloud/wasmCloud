//! P3 fixture, owner of the `token` resource. It is reached by sync calls, so
//! it runs in a companion store, and it counts its live tokens so a test can
//! see whether one was dropped.

use std::cell::RefCell;
use std::future::poll_fn;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Poll, Waker};

mod bindings {
    wit_bindgen::generate!({
        world: "owner",
        generate_all,
        async: [
            "export:wasmcloud:companion-test/tokens@0.1.0#[method]token.wait",
            "export:wasmcloud:companion-test/tokens@0.1.0#[method]token.signal",
            "export:wasmcloud:companion-test/tokens@0.1.0#make-held",
        ],
    });
}

use bindings::exports::wasmcloud::companion_test::tokens::{Guest, GuestToken, Token};
use bindings::wasi::clocks::monotonic_clock;

static LIVE: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// What `make-held` waits on and `release` sends.
    static RELEASED: Signal = Signal::default();
}

/// A one-slot mailbox between two calls on this instance.
#[derive(Default)]
struct Signal {
    message: RefCell<Option<String>>,
    waiting: RefCell<Option<Waker>>,
}

impl Signal {
    fn send(&self, message: String) {
        *self.message.borrow_mut() = Some(message);
        if let Some(waker) = self.waiting.borrow_mut().take() {
            waker.wake();
        }
    }

    fn poll_recv(&self, waker: &Waker) -> Poll<String> {
        match self.message.borrow_mut().take() {
            Some(message) => Poll::Ready(message),
            None => {
                *self.waiting.borrow_mut() = Some(waker.clone());
                Poll::Pending
            }
        }
    }
}

struct Component;

pub struct TokenState {
    signal: Signal,
}

impl TokenState {
    fn create() -> Self {
        LIVE.fetch_add(1, Ordering::SeqCst);
        Self {
            signal: Signal::default(),
        }
    }
}

impl Drop for TokenState {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

impl GuestToken for TokenState {
    fn new(_name: String) -> Self {
        Self::create()
    }

    async fn wait(&self) -> String {
        poll_fn(|cx| self.signal.poll_recv(cx.waker())).await
    }

    async fn signal(&self, msg: String) {
        self.signal.send(msg);
    }
}

impl Guest for Component {
    type Token = TokenState;

    async fn make_held(_name: String) -> Token {
        let token = Token::new(TokenState::create());
        poll_fn(|cx| RELEASED.with(|released| released.poll_recv(cx.waker()))).await;
        token
    }

    fn release() {
        RELEASED.with(|released| released.send(String::new()));
    }

    fn live() -> u32 {
        LIVE.load(Ordering::SeqCst)
    }

    fn hold(ms: u32) {
        let until = monotonic_clock::now() + u64::from(ms) * 1_000_000;
        while monotonic_clock::now() < until {}
    }
}

bindings::export!(Component with_types_in bindings);

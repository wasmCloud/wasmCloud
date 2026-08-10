//! A service exporting BOTH `wasmcloud:messaging/handler` revisions at once.
//!
//! Each handler reports a distinguishable marker through its error result —
//! `v2:{subject}` from the sync `@0.2.0` export, `v3:{subject}` (as the
//! `other` disposition) from the async `@0.3.0` one — so a test observing a
//! delivery outcome can prove WHICH revision the host invoked. The trigger
//! service is p3-only: `@0.3.0` must win, `@0.2.0` must never run, and the
//! host warns that the sync export is ignored.

use crate::bindings::exports::wasi::cli::run::Guest as RunGuest;
use crate::bindings::exports::wasmcloud::messaging0_2_0::handler::Guest as MsgGuestV2;
use crate::bindings::exports::wasmcloud::messaging0_3_0::handler::Guest as MsgGuestV3;
use crate::bindings::wasmcloud::messaging0_2_0::types::BrokerMessage as BrokerMessageV2;
use crate::bindings::wasmcloud::messaging0_3_0::types::{
    BrokerMessage as BrokerMessageV3, HandleMessageError,
};

mod bindings {
    use crate::Component;

    wit_bindgen::generate!({
        world: "dual-handler",
        generate_all
    });

    export!(Component);
}

struct Component;

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        // Nothing to co-drive; the instance is held open by the ingress serve
        // loops. The export itself is required — see wit/world.wit.
        Ok(())
    }
}


impl MsgGuestV2 for Component {
    fn handle_message(msg: BrokerMessageV2) -> Result<(), String> {
        // Must never run under a p3-only trigger service; if it does, the test
        // sees `v2:` instead of `other: v3:` and fails loudly.
        Err(format!("v2:{}", msg.subject))
    }
}

impl MsgGuestV3 for Component {
    async fn handle_message(msg: BrokerMessageV3) -> Result<(), HandleMessageError> {
        drop(msg.body);
        Err(HandleMessageError::Other(format!("v3:{}", msg.subject)))
    }
}

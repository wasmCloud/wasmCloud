use wascc_host::{
    BusDispatcher, Invocation, InvocationResponse, LatticeProvider, Result, WasccEntity,
};

pub struct NatsLatticeProvider {
    ns_prefix: Option<String>,
    dispatcher: Option<BusDispatcher>,
}

impl NatsLatticeProvider {
    pub fn new(ns_prefix: Option<String>) -> NatsLatticeProvider {
        NatsLatticeProvider {
            ns_prefix,
            dispatcher: None,
        }
    }
}

impl LatticeProvider for NatsLatticeProvider {
    fn init(&mut self, dispatcher: BusDispatcher) {
        self.dispatcher = Some(dispatcher);
    }

    fn name(&self) -> String {
        "NATS".to_string()
    }

    fn rpc(&self, inv: &Invocation) -> Result<InvocationResponse> {
        Ok(InvocationResponse::success(inv, vec![]))
    }

    fn register_rpc_listener(&self, subscriber: &WasccEntity) -> Result<()> {
        // TODO: create a subscription on the NATS subject (prefix).wasmbus.rpc.(entity_id)
        // NATS subscriber should deserialize the RPC message into an invocation, and then
        // use the dispatcher to invoke functions on the bus

        Ok(())
    }
}

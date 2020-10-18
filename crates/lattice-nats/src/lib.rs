use wascc_host::{LatticeProvider, Invocation, InvocationResponse};

type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error>>;

pub struct NatsLatticeProvider {
    ns_prefix: Option<String>,
}

impl NatsLatticeProvider {
    pub fn new(ns_prefix: Option<String>) -> NatsLatticeProvider {
        NatsLatticeProvider { ns_prefix }
    }
}

impl LatticeProvider for NatsLatticeProvider {
    fn name(&self) -> String {
        "NATS".to_string()
    }

    fn rpc(&self, inv: &Invocation) -> Result<InvocationResponse> {
        Ok(InvocationResponse::success(inv, vec![]))
    }

    fn register_interest(&self, subscriber: &WasccEntity) -> Result<()> {
        // TODO: create a subscription on the NATS subject (prefix).wasmbus.rpc.(entity_id)
    }
}

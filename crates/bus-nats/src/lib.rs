use wascc_host::MessageBusProvider;

type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error>>;

pub struct NatsBusProvider {
    ns_prefix: Option<String>,
}

impl NatsBusProvider {
    pub fn new(ns_prefix: Option<String>) -> NatsBusProvider {
        NatsBusProvider { ns_prefix }
    }
}

impl MessageBusProvider for NatsBusProvider {
    fn name(&self) -> String {
        "NATS".to_string()
    }

    fn init(&self) -> Result<()> {
        Ok(())
    }
}

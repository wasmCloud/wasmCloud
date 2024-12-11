use testcontainers::{core::WaitFor, Image};

#[derive(Default, Debug, Clone)]
pub struct LocalStack {
    _priv: (),
}

impl Image for LocalStack {
    fn name(&self) -> &str {
        "localstack/localstack"
    }

    fn tag(&self) -> &str {
        "3.7.0"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("Ready."), WaitFor::millis(3000)]
    }
}

use wasmcloud_actor::wasmcloud::messaging;

pub fn run_test() {
    messaging::consumer::publish(&messaging::types::BrokerMessage {
        subject: "interfaces-handler-reactor".into(),
        body: "test".into(),
        reply_to: Some("interfaces-reactor".to_string()),
    })
    .expect("failed to publish message")
}

impl crate::exports::wasmcloud::messaging::handler::Guest for crate::Actor {
    fn handle_message(
        messaging::types::BrokerMessage {
            subject,
            body,
            reply_to,
        }: messaging::types::BrokerMessage,
    ) -> Result<(), String> {
        assert_eq!(subject, "interfaces-reactor");
        assert_eq!(body, b"test");
        assert_eq!(reply_to.as_deref(), Some("interfaces-handler-reactor"));
        Ok(())
    }
}

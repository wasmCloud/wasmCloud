use wasmcloud_component::wasmcloud::{messaging0_2_0, messaging0_3_0};

pub fn run_test() {
    messaging0_2_0::consumer::publish(&messaging0_2_0::types::BrokerMessage {
        subject: "interfaces-handler-reactor".into(),
        body: "test".into(),
        reply_to: Some("interfaces-reactor".to_string()),
    })
    .expect("failed to publish message");

    let client = messaging0_3_0::types::Client::connect("").expect("failed to connect");
    let msg = messaging0_3_0::types::Message::new(b"test");
    messaging0_3_0::producer::send(&client, &String::from("interfaces-handler-reactor"), msg)
        .expect("failed to send message");
}

impl crate::exports::wasmcloud::messaging0_2_0::handler::Guest for crate::Actor {
    fn handle_message(
        messaging0_2_0::types::BrokerMessage {
            subject,
            body,
            reply_to,
        }: messaging0_2_0::types::BrokerMessage,
    ) -> Result<(), String> {
        assert_eq!(subject, "interfaces-reactor");
        assert_eq!(body, b"test");
        assert_eq!(reply_to.as_deref(), Some("interfaces-handler-reactor"));
        Ok(())
    }
}

impl crate::exports::wasmcloud::messaging0_3_0::incoming_handler::Guest for crate::Actor {
    fn handle(message: messaging0_3_0::types::Message) -> Result<(), messaging0_3_0::types::Error> {
        assert_eq!(message.topic(), "interfaces-reactor");
        assert_eq!(message.data(), b"test");
        Ok(())
    }
}

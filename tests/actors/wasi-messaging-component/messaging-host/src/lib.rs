#![cfg(target_arch = "wasm32")]

wit_bindgen::generate!({
    world: "host",
    path: "../wit",
});

struct Host;

impl combined::Combined for Host {
    fn publish(
        b: combined::Broker,
        c: combined::Channel,
        combined::Event {
            specversion,
            ty,
            source,
            id,
            data,
            datacontenttype,
            dataschema,
            subject,
            time,
            extensions,
        }: combined::Event,
    ) -> Result<(), combined::Error> {
        assert_eq!(b, 42);

        // From https://github.com/danbugs/wasi-messaging-demo/blob/5fa4e5ae95ee2a864fe005359e5f637f895d36fe/guest/src/lib.rs#L24-L35
        match c {
            combined::Channel::Topic(topic) => assert_eq!(topic, "rust"),
            _ => panic!("unexpected channel {c:?}"),
        }
        assert_eq!(specversion, "1.0");
        assert_eq!(ty, "com.my-messaing.rust.fizzbuzz"); // note the typo
        assert_eq!(source, "rust");
        assert_eq!(id, "123");
        assert_eq!(datacontenttype, None);
        assert_eq!(dataschema, None);
        assert_eq!(subject, None);
        assert_eq!(time, None);
        assert_eq!(extensions, None);

        host::host_call(
            "default",
            "WasiMessaging",
            "Producer.Publish",
            data.as_ref().map(Vec::as_slice),
        )
        .expect("failed call `Producer.Publish` in the host");
        Ok(())
    }

    fn subscribe(
        b: combined::Broker,
        c: combined::Channel,
    ) -> Result<combined::SubscriptionToken, combined::Error> {
        host::host_call(
            "default",
            "WasiMessaging",
            "Consumer.Subscribe",
            Some(format!("{b:?} {c:?}").as_bytes()),
        )
        .expect("failed call `Consumer.Subscribe` in the host");
        Ok("token".into())
    }

    fn unsubscribe(
        b: combined::Broker,
        st: combined::SubscriptionToken,
    ) -> Result<(), combined::Error> {
        host::host_call(
            "default",
            "WasiMessaging",
            "Consumer.Unsubscribe",
            Some(format!("{b:?} {st:?}").as_bytes()),
        )
        .expect("failed call `Consumer.Unsubscribe` in the host");
        Ok(())
    }

    fn trace(e: combined::Error) -> String {
        format!("Error code {e}")
    }

    fn drop_error(_e: combined::Error) {}

    fn open_broker(_name: String) -> Result<combined::Broker, combined::Error> {
        Ok(42)
    }

    fn drop_broker(b: combined::Broker) {
        assert_eq!(b, 42);
    }
}

export_host!(Host);

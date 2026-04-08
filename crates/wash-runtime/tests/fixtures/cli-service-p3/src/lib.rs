mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::cli::run::Guest;

struct Component;

impl Guest for Component {
    async fn run() -> Result<(), ()> {
        let msg = b"p3 service running\n".to_vec();
        let (mut tx, rx) = bindings::wit_stream::new();

        wit_bindgen::spawn(async move {
            tx.write_all(msg).await;
            drop(tx);
        });

        bindings::wasi::cli::stderr::write_via_stream(rx)
            .await
            .map_err(|_| ())?;

        Ok(())
    }
}

bindings::export!(Component with_types_in bindings);

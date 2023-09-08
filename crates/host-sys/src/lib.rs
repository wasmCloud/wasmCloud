use wasmcloud_host::wasmbus::{Host, HostConfig};

// TODO: Proper error handling

#[no_mangle]
pub extern "C" fn run_host() {
    tokio::runtime::Runtime::new()
        .expect("failed to create runtime")
        .block_on(async {
            let (host, shutdown) = Host::new(HostConfig::default())
                .await
                .expect("failed to initialize host");
            host.stopped().await.expect("failed to await host stop");
            shutdown.await.expect("failed to shutdown host");
        })
}

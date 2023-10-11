use wasmcloud_provider_httpclient::HttpClientProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // start_provider initializes the threaded tokio executor,
    // listens to lattice rpcs, handles actor links,
    // and returns only when it receives a shutdown message
    wasmcloud_provider_sdk::start_provider(
        HttpClientProvider{},
        Some("HttpClient Provider".to_string()),
    )?;

    eprintln!("HttpClient provider exiting");
    Ok(())
}

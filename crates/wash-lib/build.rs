use std::time::Duration;

use tokio::join;
use tokio::net::{lookup_host, TcpStream};
use tokio::time::timeout;

async fn record_reachability(host: &str) {
    let host_tag = host.replace('.', "_");
    // NOTE: In Rust 1.80.0 custom cfg conditions became a warning.
    //
    // As wash does not have an MSRV, we may have people build on versions *before* 1.80,
    // and we can remove the double-configuration line below a couple versions later.
    println!("cargo:rustc-check-cfg=cfg(can_reach_{host_tag})");

    if let Ok(Ok(addrs)) = timeout(Duration::from_secs(1), lookup_host(format!("{host}:443"))).await
    {
        for addr in addrs {
            if let Ok(Ok(_)) = timeout(Duration::from_millis(500), TcpStream::connect(addr)).await {
                println!("cargo:rustc-cfg=can_reach_{host_tag}");
                return;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    // Determine whether default docker is available
    println!("cargo:rustc-check-cfg=cfg(docker_available)");
    if testcontainers::core::client::docker_client_instance()
        .await
        .is_ok()
    {
        println!("cargo:rustc-cfg=docker_available");
    }

    join!(
        record_reachability("github.com"),
        record_reachability("raw.githubusercontent.com"),
    );
}

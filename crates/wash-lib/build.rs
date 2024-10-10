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
    // Determine whether docker sock is present locally
    println!("cargo:rustc-check-cfg=cfg(var_run_docker_sock_is_present)");
    if tokio::fs::metadata("/var/run/docker.sock")
        .await
        .is_ok_and(|m| m.is_file())
    {
        println!("cargo:rustc-cfg=var_run_docker_sock_is_present");
    }

    join!(
        record_reachability("github.com"),
        record_reachability("raw.githubusercontent.com"),
    );
}

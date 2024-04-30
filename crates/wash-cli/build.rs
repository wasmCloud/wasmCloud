use std::time::Duration;

use tokio::join;
use tokio::net::{lookup_host, TcpStream};
use tokio::time::timeout;

async fn record_reachability(host: &str) {
    let Ok(Ok(addrs)) = timeout(Duration::from_secs(1), lookup_host(format!("{host}:443"))).await
    else {
        return;
    };
    for addr in addrs {
        if matches!(
            timeout(Duration::from_millis(500), TcpStream::connect(addr)).await,
            Ok(Ok(_))
        ) {
            println!("cargo:rustc-cfg=can_reach_{}", host.replace('.', "_"));
            return;
        }
    }
}

#[tokio::main]
async fn main() {
    join!(
        record_reachability("github.com"),
        record_reachability("ghcr.io"),
        record_reachability("wasmcloud.azurecr.io"),
    );
}

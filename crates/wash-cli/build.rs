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
    // On Windows, `clap` can overflow the stack in debug mode due to small default stack size
    //
    // This generally triggers during the `integration_help_subcommand_check` which checks the help
    // command (i.e. walking the entire arg tree)
    //
    // see:
    // - https://github.com/clap-rs/clap/issues/5134
    // - https://github.com/clap-rs/clap/issues/4516
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        println!("cargo:rustc-link-arg=/stack:{}", 8 * 1024 * 1024);
    }

    join!(
        record_reachability("github.com"),
        record_reachability("ghcr.io"),
        record_reachability("wasmcloud.azurecr.io"),
    );
}

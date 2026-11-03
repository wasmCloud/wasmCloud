#![no_main]

use std::net::SocketAddr;

use wash_runtime::host::allowed_hosts::AllowedHost;
use wash_runtime_fuzz::fuzz_target;

fuzz_target!(|data: &str| {
    let (entry, destination) = data.split_once('\n').unwrap_or((data, data));
    if let Ok(policy) = entry.parse::<AllowedHost>() {
        if let Ok(uri) = destination.parse::<http::Uri>() {
            let _ = policy.matches(&uri);
        }
        if let Ok(addr) = destination.parse::<SocketAddr>() {
            let _ = policy.permits_addr(addr);
            let _ = policy.permits_client_addr(addr);
        }
        for port in [0, 80, 443, 4222, 65535] {
            let _ = policy.permits_name(destination, port);
            let _ = policy.permits_client_name(destination, port);
        }
        let _ = policy.socket_port();
        let _ = policy.client_port();
    }
});

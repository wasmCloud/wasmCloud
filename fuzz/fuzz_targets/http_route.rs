#![no_main]

use libfuzzer_sys::fuzz_target;
use wash_runtime::host::http::{
    NormalizedPathPrefix, parse_local_route, release_fuzz::fuzz_route_matcher,
};

fuzz_target!(|data: &str| {
    let mut fields = data.splitn(3, '\n');
    let entry = fields.next().unwrap_or_default();
    let host = fields.next().unwrap_or(entry);
    let path = fields.next().unwrap_or("/");
    let prefix = NormalizedPathPrefix::new(entry);
    let _ = prefix.matches(path);
    if let Some(route) = parse_local_route(entry) {
        fuzz_route_matcher(entry, &route.host, path);
        fuzz_route_matcher(entry, &route.host.to_ascii_uppercase(), path);
        fuzz_route_matcher(entry, &format!("{}:8080", route.host), path);
    }
    fuzz_route_matcher(entry, host, path);
});

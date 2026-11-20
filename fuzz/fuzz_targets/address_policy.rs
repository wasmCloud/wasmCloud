#![no_main]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use wash_runtime::host::egress_policy::EgressAddressPolicy;
use wash_runtime_fuzz::fuzz_target;

fn check(addr: IpAddr) {
    for deny_special in [false, true] {
        for allow_private in [false, true] {
            let policy = EgressAddressPolicy {
                deny_special,
                allow_private,
            };
            assert_eq!(policy.permits(addr), policy.permits(addr.to_canonical()));
            if let IpAddr::V4(v4) = addr {
                assert_eq!(
                    policy.permits(addr),
                    policy.permits(v4.to_ipv6_mapped().into())
                );
            }
            if !deny_special && allow_private {
                assert!(policy.permits(addr));
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if let Some(octets) = data.get(..4) {
        check(Ipv4Addr::from(<[u8; 4]>::try_from(octets).unwrap()).into());
    }
    if let Some(octets) = data.get(..16) {
        check(Ipv6Addr::from(<[u8; 16]>::try_from(octets).unwrap()).into());
    }
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(addr) = text.parse::<IpAddr>() {
            check(addr);
        }
    }
});

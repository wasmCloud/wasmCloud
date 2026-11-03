#![no_main]

use wash_runtime::{washlet::types::v2, wit::WitInterface};
use wash_runtime_fuzz::fuzz_target;
use wash_runtime_fuzz::{check_interface, check_runtime_interface};

fuzz_target!(|data: &str| {
    let (left, right) = data.split_once('\n').unwrap_or((data, data));
    let mut interface = WitInterface::from(left);
    let other = WitInterface::from(right);
    let _ = interface.contains(&other);
    let _ = other.contains(&interface);
    let _ = interface.merge(&other);
    let _ = interface.instance();
    let _ = interface.to_string();
    if let Ok(interfaces) = serde_json::from_str::<Vec<v2::WitInterface>>(data) {
        for interface in interfaces {
            check_interface(interface);
        }
    }
    if let Ok(interface) = serde_json::from_str::<v2::WitInterface>(data) {
        check_interface(interface);
    }
    if let Ok(interfaces) = serde_json::from_str::<Vec<WitInterface>>(data) {
        for interface in interfaces {
            check_runtime_interface(interface);
        }
    }
});

mod fuzzer;

use wash_runtime::washlet::types::v2;

pub fn check_workload(workload: v2::Workload) {
    for volume in workload.volumes {
        let _: wash_runtime::types::Volume = volume.into();
    }
    if let Some(service) = workload.service {
        if let Some(resources) = service.local_resources {
            let _ = wash_runtime::types::LocalResources::try_from(resources);
        }
    }
    if let Some(world) = workload.wit_world {
        for component in world.components {
            if let Some(resources) = component.local_resources {
                let _ = wash_runtime::types::LocalResources::try_from(resources);
            }
        }
        for interface in world.host_interfaces {
            check_interface(interface);
        }
    }
}

pub fn check_interface(interface: v2::WitInterface) {
    check_runtime_interface(interface.into());
}

pub fn check_runtime_interface(interface: wash_runtime::wit::WitInterface) {
    let encoded = serde_json::to_vec(&interface).unwrap();
    let decoded = serde_json::from_slice::<wash_runtime::wit::WitInterface>(&encoded).unwrap();
    assert_eq!(interface, decoded);
    let _ = interface.instance();
    let _ = interface.to_string();
    let _ = interface.is_host_builtin();
    let _ = interface.is_incoming_http_handler();
}

#![no_main]

use prost::Message;
use wash_runtime::washlet::types::v2;
use wash_runtime_fuzz::fuzz_target;
use wash_runtime_fuzz::{check_interface, check_workload};

fn roundtrip<T: Message + Default + PartialEq + std::fmt::Debug>(message: &T) {
    assert_eq!(
        *message,
        T::decode(message.encode_to_vec().as_slice()).unwrap()
    );
}

fuzz_target!(|data: &[u8]| {
    if let Ok(request) = v2::WorkloadStartRequest::decode(data) {
        roundtrip(&request);
        if let Some(workload) = request.workload {
            check_workload(workload);
        }
    }
    if let Ok(workload) = v2::Workload::decode(data) {
        roundtrip(&workload);
        check_workload(workload);
    }
    if let Ok(resources) = v2::LocalResources::decode(data) {
        roundtrip(&resources);
        let _ = wash_runtime::types::LocalResources::try_from(resources);
    }
    if let Ok(interface) = v2::WitInterface::decode(data) {
        roundtrip(&interface);
        check_interface(interface);
    }
    let _ = v2::WorkloadStopRequest::decode(data);
    let _ = v2::WorkloadStatusRequest::decode(data);
});

#![no_main]

use libfuzzer_sys::fuzz_target;
use wash_runtime::washlet::types::v2;
use wash_runtime_fuzz::check_workload;

fuzz_target!(|data: &[u8]| {
    // NATS commands use the generated protobuf JSON deserializer.
    if let Ok(request) = serde_json::from_slice::<v2::WorkloadStartRequest>(data) {
        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded = serde_json::from_slice::<v2::WorkloadStartRequest>(&encoded).unwrap();
        assert_eq!(request, decoded);
        if let Some(workload) = request.workload {
            check_workload(workload);
        }
    }
    if let Ok(workload) = serde_json::from_slice::<v2::Workload>(data) {
        check_workload(workload);
    }
    if let Ok(resources) = serde_json::from_slice::<v2::LocalResources>(data) {
        let _ = wash_runtime::types::LocalResources::try_from(resources);
    }
    let _ = serde_json::from_slice::<v2::WorkloadStopRequest>(data);
    let _ = serde_json::from_slice::<v2::WorkloadStatusRequest>(data);
});

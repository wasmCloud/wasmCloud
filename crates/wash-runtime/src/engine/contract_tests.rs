use super::*;
use crate::types::{LocalResources, Service, Volume, VolumeMount};

const CLI_RUN_SERVICE: &str =
    "(component (instance $run) (export \"wasi:cli/run@0.2.0\" (instance $run)))";
const SINGLE_INTERFACE: &str = "Service must export a single interface with the 'run' function";

fn engine() -> Engine {
    Engine::builder()
        .with_pooling_allocator(false)
        .without_max_process_memory()
        .build()
        .unwrap()
}

fn workload() -> Workload {
    Workload {
        name: "contract".into(),
        namespace: "tests".into(),
        annotations: Default::default(),
        components: vec![],
        service: None,
        host_interfaces: vec![],
        volumes: vec![],
    }
}

fn component(name: &str, bytes: &[u8]) -> crate::types::Component {
    crate::types::Component {
        name: name.into(),
        bytes: bytes.to_vec().into(),
        ..Default::default()
    }
}

fn service(bytes: &[u8]) -> Service {
    Service {
        bytes: bytes.to_vec().into(),
        digest: None,
        local_resources: LocalResources::default(),
        max_restarts: 0,
    }
}

fn empty_dir(name: &str) -> Volume {
    Volume {
        name: name.into(),
        volume_type: VolumeType::EmptyDir(EmptyDirVolume {}),
    }
}

fn host_path(path: &std::path::Path) -> Volume {
    Volume {
        name: "data".into(),
        volume_type: VolumeType::HostPath(HostPathVolume {
            local_path: path.to_string_lossy().into_owned(),
        }),
    }
}

fn metered_engine() -> (Engine, Arc<crate::host::quota::PolicyMeters>) {
    let meters = Arc::new(crate::host::quota::PolicyMeters::default());
    let engine = Engine::builder()
        .with_socket_policy(Arc::new(crate::sockets::policy::SocketPolicy {
            meters: Some(Arc::clone(&meters)),
            ..Default::default()
        }))
        .with_pooling_allocator(false)
        .without_max_process_memory()
        .build()
        .unwrap();
    (engine, meters)
}

fn walkdir(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walkdir(&path));
        }
        found.push(path);
    }
    found
}

#[test]
fn a_component_with_invalid_bytes_fails_initialization() {
    let mut input = workload();
    input.components.push(component("broken", b"invalid"));
    let Err(err) = engine().initialize_workload("invalid-component", input) else {
        panic!("invalid component bytes should be refused");
    };
    assert!(
        format!("{err:#}").contains("failed to create component from bytes"),
        "{err:#}"
    );
}

#[test]
fn a_service_with_invalid_bytes_fails_initialization() {
    let mut input = workload();
    input.service = Some(service(b"invalid"));
    let Err(err) = engine().initialize_workload("invalid-service", input) else {
        panic!("invalid service bytes should be refused");
    };
    assert!(
        format!("{err:#}").contains("failed to create component from bytes"),
        "{err:#}"
    );
}

#[test]
fn an_empty_dir_is_unavailable_when_the_scratch_root_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("file");
    std::fs::write(&blocked, b"file").unwrap();
    let engine = Engine::builder()
        .with_pooling_allocator(false)
        .with_scratch_root(&blocked)
        .without_max_process_memory()
        .build()
        .unwrap();
    let mut input = workload();
    input.volumes.push(empty_dir("scratch"));
    let Err(err) = engine.initialize_workload("blocked-scratch", input) else {
        panic!("an emptyDir without a usable scratch root should be refused");
    };
    assert!(
        format!("{err:#}").contains("emptyDir volumes are unavailable"),
        "{err:#}"
    );
}

#[test]
fn a_missing_host_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut input = workload();
    input.volumes.push(host_path(&dir.path().join("missing")));
    let Err(err) = engine().initialize_workload("missing-path", input) else {
        panic!("a missing host path should be refused");
    };
    assert!(
        format!("{err:#}").contains("does not exist or is not a directory"),
        "{err:#}"
    );
}

#[test]
fn a_host_path_naming_a_file_is_refused_before_the_policy() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, b"file").unwrap();
    let (engine, meters) = metered_engine();
    let mut input = workload();
    input.volumes.push(host_path(&file));
    let Err(err) = engine.initialize_workload("file-path", input) else {
        panic!("a host path naming a file should be refused");
    };
    assert!(
        format!("{err:#}").contains("does not exist or is not a directory"),
        "{err:#}"
    );
    assert_eq!(meters.host_path_denied(), 0);
}

#[test]
fn a_start_that_fails_after_scratch_was_created_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::builder()
        .with_pooling_allocator(false)
        .with_scratch_root(dir.path())
        .without_max_process_memory()
        .build()
        .unwrap();
    let valid = wat::parse_str("(component)").unwrap();
    let mut input = workload();
    input.volumes.push(empty_dir("scratch"));
    input.components.push(component("valid", &valid));
    input.components.push(component("broken", b"invalid"));
    let Err(err) = engine.initialize_workload("failed-start", input) else {
        panic!("a workload with invalid component bytes should be refused");
    };
    assert!(
        format!("{err:#}").contains("failed to create component from bytes"),
        "{err:#}"
    );
    let left_behind: Vec<PathBuf> = walkdir(dir.path())
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("workload-"))
        })
        .collect();
    assert!(left_behind.is_empty(), "{left_behind:?}");
}

fn mounts_for(volume: &str) -> LocalResources {
    LocalResources {
        volume_mounts: vec![
            VolumeMount {
                name: volume.into(),
                mount_path: "/data".into(),
                read_only: true,
            },
            VolumeMount {
                name: "unknown".into(),
                mount_path: "/unknown".into(),
                read_only: false,
            },
        ],
        ..Default::default()
    }
}

fn opened_volumes(
    dir: &std::path::Path,
) -> std::collections::HashMap<String, volumes::OpenedVolume> {
    let opened = volumes::ReservedHostPaths::default().open(dir).unwrap();
    std::collections::HashMap::from([("data".to_string(), opened)])
}

#[test]
fn a_service_mounts_known_volumes_and_skips_unknown_ones() {
    let dir = tempfile::tempdir().unwrap();
    let volumes = opened_volumes(dir.path());
    let bytes = wat::parse_str(CLI_RUN_SERVICE).unwrap();
    let service = engine()
        .initialize_service(
            "workload",
            "contract",
            "tests",
            Service {
                local_resources: mounts_for("data"),
                max_restarts: 2,
                ..service(&bytes)
            },
            &volumes,
            Arc::default(),
        )
        .unwrap();
    assert_eq!(service.local_resources().volume_mounts.len(), 2);
    assert_eq!(service.resolved_volume_mounts.len(), 1);
    assert_eq!(service.volume_mounts[0].1.mount_path, "/data");
}

#[test]
fn a_component_mounts_known_volumes_and_skips_unknown_ones() {
    let dir = tempfile::tempdir().unwrap();
    let volumes = opened_volumes(dir.path());
    let bytes = wat::parse_str("(component)").unwrap();
    let component = engine()
        .initialize_workload_component(
            "workload",
            "contract",
            "tests",
            crate::types::Component {
                local_resources: mounts_for("data"),
                ..component("component", &bytes)
            },
            &volumes,
            Arc::default(),
        )
        .unwrap();
    assert_eq!(component.local_resources().volume_mounts.len(), 2);
    assert_eq!(component.resolved_volume_mounts.len(), 1);
    assert_eq!(component.volume_mounts[0].1.mount_path, "/data");
}

fn initialize_service(bytes: &[u8]) -> anyhow::Result<WorkloadService> {
    engine().initialize_service(
        "workload",
        "contract",
        "tests",
        service(bytes),
        &std::collections::HashMap::new(),
        Arc::default(),
    )
}

#[test]
fn a_service_exporting_wasi_cli_run_is_accepted() {
    let bytes = wat::parse_str(CLI_RUN_SERVICE).unwrap();
    let service = initialize_service(&bytes).unwrap();
    let world = service.world();
    assert_eq!(world.exports.len(), 1);
    assert!(
        world
            .exports
            .iter()
            .any(|iface| iface.package == "cli" && iface.interfaces.contains("run"))
    );
}

#[test]
fn a_service_with_no_exports_is_refused() {
    let bytes = wat::parse_str("(component)").unwrap();
    let Err(err) = initialize_service(&bytes) else {
        panic!("a service exporting nothing should be refused");
    };
    assert!(format!("{err:#}").contains(SINGLE_INTERFACE), "{err:#}");
}

#[test]
fn a_service_with_two_instance_exports_is_refused() {
    let bytes = wat::parse_str(
        "(component \
            (instance $a) (instance $b) \
            (export \"contract:one/a\" (instance $a)) \
            (export \"contract:two/b\" (instance $b)))",
    )
    .unwrap();
    let Err(err) = initialize_service(&bytes) else {
        panic!("a service exporting two interfaces should be refused");
    };
    assert!(format!("{err:#}").contains(SINGLE_INTERFACE), "{err:#}");
}

#[test]
fn a_service_and_a_component_with_one_digest_share_one_compile() {
    use std::sync::atomic::Ordering;
    const DIGEST: &str = "sha256:shared";

    let engine = engine();
    let bytes = wat::parse_str(CLI_RUN_SERVICE).unwrap();
    let mut input = workload();
    input.service = Some(Service {
        digest: Some(DIGEST.into()),
        ..service(&bytes)
    });
    input.components.push(crate::types::Component {
        digest: Some(DIGEST.into()),
        ..component("component", &bytes)
    });
    let workload = engine.initialize_workload("shared", input).unwrap();
    assert_eq!(engine.compiles.load(Ordering::SeqCst), 1);
    assert_eq!(engine.live_compiles.lock().unwrap().len(), 1);
    drop(workload);
    assert!(engine.live_compiles.lock().unwrap().is_empty());
}

#[test]
fn fuel_consumption_follows_the_base_config_until_the_builder_overrides_it() {
    let mut base = wasmtime::Config::default();
    base.consume_fuel(true);
    let engine = Engine::builder()
        .with_config(base.clone())
        .with_pooling_allocator(false)
        .without_max_process_memory()
        .build()
        .unwrap();
    assert!(engine.consumes_fuel());
    let engine = Engine::builder()
        .with_config(base)
        .with_fuel_consumption(false)
        .with_pooling_allocator(false)
        .without_max_process_memory()
        .build()
        .unwrap();
    assert!(!engine.consumes_fuel());
}

#[test]
fn without_max_process_memory_installs_no_guard() {
    assert!(engine().process_memory().is_none());
}

#[test]
fn a_custom_config_leaves_the_pooling_allocator_off() {
    if std::env::var_os("WASMTIME_POOLING").is_some() {
        return;
    }
    let engine = Engine::builder()
        .with_config(wasmtime::Config::default())
        .without_max_process_memory()
        .build()
        .unwrap();
    assert!(engine.pool_totals().is_none());
}

#[test]
fn every_proposal_parses_and_builds() {
    for name in [
        "component-model-async",
        "component-model-map",
        "gc",
        "exception-handling",
        "wide-arithmetic",
        "threads",
        "tail-call",
    ] {
        let proposal: WasmProposal = name.parse().unwrap_or_else(|e| panic!("{name}: {e}"));
        Engine::builder()
            .with_wasm_proposal(proposal)
            .with_pooling_allocator(false)
            .without_max_process_memory()
            .build()
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
}

#[test]
fn an_unknown_proposal_is_named_in_its_parse_error() {
    let Err(err) = "nonsense".parse::<WasmProposal>() else {
        panic!("an unknown proposal should not parse");
    };
    let rendered = err.to_string();
    assert!(
        rendered.contains("unknown wasm proposal \"nonsense\"; expected one of:"),
        "{rendered}"
    );
}

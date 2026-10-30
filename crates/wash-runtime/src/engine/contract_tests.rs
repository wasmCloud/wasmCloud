use super::*;
use crate::types::{LocalResources, Service, Volume, VolumeMount};

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

#[test]
fn initialization_names_invalid_components_services_and_unavailable_scratch() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("file");
    std::fs::write(&blocked, b"file").unwrap();
    let engine = EngineBuilder::new()
        .with_pooling_allocator(false)
        .with_scratch_root(&blocked)
        .without_max_process_memory()
        .build()
        .unwrap();
    let mut input = workload();
    input.components.push(crate::types::Component {
        name: "broken".into(),
        bytes: b"invalid".as_slice().into(),
        ..Default::default()
    });
    let error = engine
        .initialize_workload("invalid-component", input)
        .err()
        .expect("initialization must fail");
    assert!(format!("{error:#}").contains("failed to create component from bytes"));
    let mut input = workload();
    input.service = Some(Service {
        bytes: b"invalid".as_slice().into(),
        digest: None,
        local_resources: LocalResources::default(),
        max_restarts: 0,
    });
    let error = engine
        .initialize_workload("invalid-service", input)
        .err()
        .expect("initialization must fail");
    assert!(format!("{error:#}").contains("failed to create component from bytes"));
    let mut input = workload();
    input.volumes.push(Volume {
        name: "scratch".into(),
        volume_type: VolumeType::EmptyDir(EmptyDirVolume {}),
    });
    let error = engine
        .initialize_workload("blocked-scratch", input)
        .err()
        .expect("initialization must fail");
    assert!(
        error
            .to_string()
            .contains("emptyDir volumes are unavailable")
    );
    let mut input = workload();
    input.volumes.push(Volume {
        name: "missing".into(),
        volume_type: VolumeType::HostPath(HostPathVolume {
            local_path: dir.path().join("missing").to_string_lossy().into(),
        }),
    });
    let error = engine
        .initialize_workload("missing-path", input)
        .err()
        .expect("initialization must fail");
    assert!(
        error
            .to_string()
            .contains("does not exist or is not a directory")
    );
}

#[test]
fn initialization_resolves_service_and_component_mounts_and_skips_unknown_volumes() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::builder()
        .with_pooling_allocator(false)
        .without_max_process_memory()
        .build()
        .unwrap();
    let opened = volumes::ReservedHostPaths::default()
        .open(dir.path())
        .unwrap();
    let volumes = std::collections::HashMap::from([("data".into(), opened)]);
    let resources = LocalResources {
        volume_mounts: vec![
            VolumeMount {
                name: "data".into(),
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
    };
    let bytes = wat::parse_str(
        "(component (instance $service) (export \"contract:test/service\" (instance $service)))",
    )
    .unwrap();
    let service = engine
        .initialize_service(
            "workload",
            "contract",
            "tests",
            Service {
                bytes: bytes.into(),
                digest: None,
                local_resources: resources.clone(),
                max_restarts: 2,
            },
            &volumes,
            Arc::default(),
        )
        .unwrap();
    assert_eq!(service.local_resources().volume_mounts.len(), 2);
    assert_eq!(service.resolved_volume_mounts.len(), 1);
    assert_eq!(service.volume_mounts[0].1.mount_path, "/data");
    let component = engine
        .initialize_workload_component(
            "workload",
            "contract",
            "tests",
            crate::types::Component {
                name: "component".into(),
                bytes: wat::parse_str("(component)").unwrap().into(),
                local_resources: resources,
                ..Default::default()
            },
            &volumes,
            Arc::default(),
        )
        .unwrap();
    assert_eq!(component.resolved_volume_mounts.len(), 1);
    assert_eq!(component.volume_mounts[0].1.mount_path, "/data");
    let error = engine
        .initialize_service(
            "workload",
            "contract",
            "tests",
            Service {
                bytes: wat::parse_str("(component)").unwrap().into(),
                digest: None,
                local_resources: LocalResources::default(),
                max_restarts: 0,
            },
            &volumes,
            Arc::default(),
        )
        .err()
        .expect("initialization must fail");
    assert!(
        error
            .to_string()
            .contains("Service must export a single interface")
    );
}

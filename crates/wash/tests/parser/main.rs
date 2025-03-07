use std::{collections::HashSet, fs, path::PathBuf};

use claims::{assert_err, assert_ok};
use semver::Version;
use wash::lib::parser::{
    load_config, CommonConfig, ComponentConfig, LanguageConfig, RegistryConfig, RustConfig,
    TinyGoConfig, TinyGoGarbageCollector, TinyGoScheduler, TypeConfig, WasmTarget,
};

#[tokio::test]
async fn rust_component() {
    let result = load_config(
        Some(PathBuf::from("./tests/parser/files/rust_component.toml")),
        None,
    )
    .await;

    let config = assert_ok!(result);

    assert_eq!(
        config.language,
        LanguageConfig::Rust(RustConfig {
            cargo_path: Some("./cargo".into()),
            target_path: Some("./target".into()),
            debug: false,
        })
    );

    assert_eq!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            key_directory: PathBuf::from("./keys"),
            destination: Some(PathBuf::from("./build/testcomponent.wasm".to_string())),
            wasip1_adapter_path: None,
            wasm_target: WasmTarget::CoreModule,
            ..ComponentConfig::default()
        })
    );

    assert_eq!(
        config.common,
        CommonConfig {
            name: "testcomponent".to_string(),
            version: Version::parse("0.1.0").unwrap(),
            revision: 0,
            project_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap(),
            build_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap()
                .join("build"),
            wit_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap()
                .join("wit"),
            wasm_bin_name: None,
            registry: RegistryConfig::default(),
        }
    );
}

#[tokio::test]
/// When given a specific toml file's path, it should parse the file and return a `ProjectConfig`.
async fn rust_component_with_revision() {
    let result = load_config(
        Some(PathBuf::from(
            "./tests/parser/files/rust_component_with_revision.toml",
        )),
        None,
    )
    .await;

    let config = assert_ok!(result);

    assert_eq!(
        config.language,
        LanguageConfig::Rust(RustConfig {
            cargo_path: Some("./cargo".into()),
            target_path: Some("./target".into()),
            debug: false,
        })
    );

    assert_eq!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            key_directory: PathBuf::from("./keys"),
            destination: Some(PathBuf::from("./build/testcomponent.wasm".to_string())),
            wasip1_adapter_path: None,
            wasm_target: WasmTarget::CoreModule,
            wit_world: None,
            ..ComponentConfig::default()
        })
    );

    assert_eq!(
        config.common,
        CommonConfig {
            name: "testcomponent".to_string(),
            version: Version::parse("0.1.0").unwrap(),
            revision: 666,
            project_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap(),
            build_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap()
                .join("build"),
            wit_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap()
                .join("wit"),
            wasm_bin_name: None,
            registry: RegistryConfig::default(),
        }
    );
}

#[tokio::test]
async fn tinygo_component_module_scheduler_gc() {
    let result = load_config(
        Some(PathBuf::from(
            "./tests/parser/files/tinygo_component_scheduler_gc.toml",
        )),
        None,
    )
    .await;

    let config = assert_ok!(result);

    assert_eq!(
        config.language,
        LanguageConfig::TinyGo(TinyGoConfig {
            tinygo_path: None,
            disable_go_generate: false,
            scheduler: Some(TinyGoScheduler::None),
            garbage_collector: Some(TinyGoGarbageCollector::Leaking),
        })
    );
}

#[tokio::test]
async fn tinygo_component_module() {
    let result = load_config(
        Some(PathBuf::from(
            "./tests/parser/files/tinygo_component_module.toml",
        )),
        None,
    )
    .await;

    let config = assert_ok!(result);

    assert_eq!(
        config.language,
        LanguageConfig::TinyGo(TinyGoConfig {
            tinygo_path: Some("path/to/tinygo".into()),
            disable_go_generate: false,
            scheduler: None,
            garbage_collector: None,
        })
    );

    assert_eq!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            key_directory: PathBuf::from("./keys"),
            destination: Some(PathBuf::from("./build/testcomponent.wasm".to_string())),
            wasip1_adapter_path: None,
            wasm_target: WasmTarget::CoreModule,
            ..ComponentConfig::default()
        })
    );

    assert_eq!(
        config.common,
        CommonConfig {
            name: "testcomponent".to_string(),
            version: Version::parse("0.1.0").unwrap(),
            revision: 0,
            project_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap(),
            build_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap()
                .join("build"),
            wit_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap()
                .join("wit"),
            wasm_bin_name: None,
            registry: RegistryConfig::default(),
        }
    );
}

#[tokio::test]
async fn tinygo_component() {
    let result = load_config(
        Some(PathBuf::from("./tests/parser/files/tinygo_component.toml")),
        None,
    )
    .await;

    let config = assert_ok!(result);

    assert_eq!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            key_directory: PathBuf::from("./keys"),
            destination: Some(PathBuf::from("./build/testcomponent.wasm".to_string())),
            wasip1_adapter_path: None,
            wasm_target: WasmTarget::WasiP2,
            ..ComponentConfig::default()
        })
    );
}

#[tokio::test]
/// When given a folder, should automatically grab a wasmcloud.toml file inside it and parse it.
async fn folder_path() {
    let result = load_config(Some(PathBuf::from("./tests/parser/files/folder")), None).await;

    let config = assert_ok!(result);

    assert_eq!(
        config.language,
        LanguageConfig::Rust(RustConfig {
            cargo_path: Some("./cargo".into()),
            target_path: Some("./target".into()),
            debug: false,
        })
    );
}

/// Gets the full path of a local path. Test helper.
fn get_full_path(path: &str) -> String {
    match fs::canonicalize(path) {
        Ok(path) => path.to_str().unwrap().to_string(),
        Err(_) => panic!("get_full_path helper error. Could not find path: {path}"),
    }
}

#[tokio::test]
/// When given a folder with no wasmcloud.toml file, should return an error.
async fn folder_path_with_no_config() {
    let result = load_config(Some(PathBuf::from("./tests/parser/files/noconfig")), None).await;

    let err = assert_err!(result);
    assert_eq!(
        format!(
            "failed to find wasmcloud.toml in [{}]",
            get_full_path("./tests/parser/files/noconfig")
        ),
        err.to_string().as_str()
    );
}

#[tokio::test]
/// When given a random file, should return an error.
async fn random_file() {
    let result = load_config(Some(PathBuf::from("./tests/parser/files/random.txt")), None).await;

    let err = assert_err!(result);
    assert_eq!(
        format!(
            "invalid config file: {}",
            get_full_path("./tests/parser/files/random.txt")
        ),
        err.to_string().as_str()
    );
}

#[tokio::test]
/// When given a nonexistent file or path, should return an error.
async fn nonexistent_file() {
    let result = load_config(
        Some(PathBuf::from("./tests/parser/files/nonexistent.toml")),
        None,
    )
    .await;

    let err = assert_err!(result);
    assert_eq!(
        "path ./tests/parser/files/nonexistent.toml does not exist",
        err.to_string().as_str()
    );
}

#[tokio::test]
async fn nonexistent_folder() {
    let result = load_config(
        Some(PathBuf::from("./tests/parser/files/nonexistent/")),
        None,
    )
    .await;

    let err = assert_err!(result);
    assert_eq!(
        "path ./tests/parser/files/nonexistent/ does not exist",
        err.to_string().as_str()
    );
}

#[tokio::test]
async fn minimal_rust_component() {
    let result = load_config(
        Some(PathBuf::from(
            "./tests/parser/files/minimal_rust_component.toml",
        )),
        None,
    )
    .await;

    let config = assert_ok!(result);

    let mut expected_key_dir =
        etcetera::home_dir().expect("Unable to determine the user's home directory");
    expected_key_dir.push(".wash/keys");

    assert_eq!(
        config.language,
        LanguageConfig::Rust(RustConfig {
            cargo_path: None,
            target_path: None,
            debug: false,
        })
    );

    assert_eq!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            key_directory: expected_key_dir,
            destination: None,
            wasip1_adapter_path: None,
            wasm_target: WasmTarget::CoreModule,
            ..ComponentConfig::default()
        })
    );

    assert_eq!(
        config.common,
        CommonConfig {
            name: "testcomponent".to_string(),
            version: Version::parse("0.1.0").unwrap(),
            project_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap(),
            build_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap()
                .join("build"),
            wit_dir: PathBuf::from("./tests/parser/files/")
                .canonicalize()
                .unwrap()
                .join("wit"),
            revision: 0,
            wasm_bin_name: None,
            registry: RegistryConfig::default(),
        }
    );
}

#[tokio::test]
async fn cargo_toml_component() {
    let result = load_config(
        Some(PathBuf::from(
            "./tests/parser/files/withcargotoml/minimal_rust_component_with_cargo.toml",
        )),
        None,
    )
    .await;

    let config = assert_ok!(result);

    let mut expected_key_dir =
        etcetera::home_dir().expect("Unable to determine the user's home directory");
    expected_key_dir.push(".wash/keys");

    assert_eq!(
        config.language,
        LanguageConfig::Rust(RustConfig {
            cargo_path: None,
            target_path: None,
            debug: false,
        })
    );

    assert_eq!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            key_directory: expected_key_dir,
            destination: None,
            wasip1_adapter_path: None,
            wasm_target: WasmTarget::CoreModule,
            ..ComponentConfig::default()
        })
    );

    assert_eq!(
        config.common,
        CommonConfig {
            name: "withcargotoml".to_string(),
            version: Version::parse("0.200.0").unwrap(),
            project_dir: PathBuf::from("./tests/parser/files/withcargotoml")
                .canonicalize()
                .unwrap(),
            build_dir: PathBuf::from("./tests/parser/files/withcargotoml")
                .canonicalize()
                .unwrap()
                .join("build"),
            wit_dir: PathBuf::from("./tests/parser/files/withcargotoml")
                .canonicalize()
                .unwrap()
                .join("wit"),
            revision: 0,
            wasm_bin_name: None,
            registry: RegistryConfig::default(),
        }
    );
}

/// wasm_target=wasm32-wasip2 is properly parsed
/// see: https://github.com/wasmCloud/wash/issues/640
#[tokio::test]
async fn minimal_rust_component_p2() {
    let result = load_config(
        Some(PathBuf::from(
            "./tests/parser/files/minimal_rust_component_wasip2.toml",
        )),
        None,
    )
    .await;

    let config = assert_ok!(result);

    let mut expected_default_key_dir =
        etcetera::home_dir().expect("Unable to determine the user's home directory");
    expected_default_key_dir.push(".wash/keys");

    assert_eq!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            key_directory: expected_default_key_dir,
            wasm_target: WasmTarget::WasiP2,
            wit_world: Some("test-world".to_string()),
            ..Default::default()
        })
    );
}

/// wasm_target=wasm32-wasip1 is properly parsed
/// see: https://github.com/wasmCloud/wash/issues/640
#[tokio::test]
async fn minimal_rust_component_wasip1() {
    let result = load_config(
        Some(PathBuf::from(
            "./tests/parser/files/minimal_rust_component_wasip1.toml",
        )),
        None,
    )
    .await;

    let config = assert_ok!(result);
    assert!(matches!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            wasm_target: WasmTarget::WasiP1,
            ..
        })
    ));
}

/// wasm_target=wasm32-unknown-unknown is properly parsed
/// see: https://github.com/wasmCloud/wash/issues/640
#[tokio::test]
async fn minimal_rust_component_core_module() {
    let result = load_config(
        Some(PathBuf::from(
            "./tests/parser/files/minimal_rust_component_core_module.toml",
        )),
        None,
    )
    .await;

    let config = assert_ok!(result);
    assert!(matches!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            wasm_target: WasmTarget::CoreModule,
            ..
        })
    ));
}

/// Tags are properly handled (duplicates, pre-existing experimental tag)
/// see: https://github.com/wasmCloud/wash/pull/951
#[tokio::test]
async fn tags() {
    let result = load_config(Some(PathBuf::from("./tests/parser/files/tags.toml")), None).await;

    let config = assert_ok!(result);
    assert!(matches!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            tags,
            ..
        }) if tags == Some(HashSet::from(["test".into(), "wasmcloud.com/experimental".into()])),
    ));
}

/// Projects with overridden paths should be properly handled
///
/// NOTE: this test uses hard-coded paths in config that include '/tmp'
/// and as such, doesn't work on windows.
#[tokio::test]
#[cfg(not(target_os = "windows"))]
async fn separate_project_paths() {
    let result = load_config(
        Some(PathBuf::from(
            "./tests/parser/files/separate_project_paths.toml",
        )),
        None,
    )
    .await;
    let config = assert_ok!(result);
    // Different project path handled
    assert_eq!(
        config.common.project_dir,
        PathBuf::from("./tests/parser")
            .canonicalize()
            .expect("failed to canonicalize test path")
    );
    // Absolute paths properly handled
    assert_eq!(
        config.common.build_dir,
        PathBuf::from("/tmp/some/other/build")
    );
    assert_eq!(config.common.wit_dir, PathBuf::from("/tmp/nested/wit"));

    // Relative paths properly handled
    assert!(matches!(
        config.project_type,
        TypeConfig::Component(ComponentConfig {
            build_artifact: Some(build_artifact),
            destination: Some(destination),
            ..
        }) if build_artifact == PathBuf::from("build/testcomponent_raw.wasm")
        && destination == PathBuf::from("./build/testcomponent.wasm")
    ));

    assert!(matches!(
        config.language,
        LanguageConfig::Rust(RustConfig {
            cargo_path: Some(cargo_path),
            target_path: Some(target_path),
            debug: false,
        }) if cargo_path == PathBuf::from("../cargo")
        && target_path == PathBuf::from("./target")
    ));
}

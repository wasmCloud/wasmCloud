use anyhow::Result;
use tokio::fs;
use wit_component::DecodedWasm;

mod common;
use common::load_fixture;

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn test_wash_wit_build() {
    let test_setup = load_fixture("wasi-http").await.unwrap();

    let status = test_setup
        .base_command()
        .args(["wit", "build", "-f", "built.wasm"])
        .status()
        .await
        .expect("Failed to build project");
    assert!(status.success(), "Failed to build project");

    let contents = fs::read(test_setup.project_dir.join("built.wasm"))
        .await
        .expect("Failed to read generated file");
    let package = wit_component::decode(&contents).expect("Failed to decode built package");
    let (resolve, id) = match package {
        DecodedWasm::WitPackage(resolve, id) => (resolve, id),
        _ => panic!("Expected a package"),
    };
    assert_eq!(
        resolve.packages.get(id).unwrap().name.to_string(),
        "wasi:http@0.2.0",
        "Should have encoded the correct package"
    );
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn test_wash_wit_deps_and_build() {
    let test_setup = load_fixture("dog-fetcher").await.unwrap();

    // Run `wit deps`
    let fetch_status = test_setup
        .base_command()
        .args(["wit", "deps"])
        .status()
        .await
        .expect("Failed to fetch dependencies");
    assert!(fetch_status.success(), "Failed to fetch dependencies");

    // NOTE: we don't need to check all the deps as that is tested in the upstream library. This is
    // a smoke test to make sure we can fully fetch and build a project.

    // Run `cargo build`
    let build_status = test_setup
        .base_command()
        .arg("build")
        .status()
        .await
        .expect("Failed to execute cargo build");
    assert!(
        build_status.success(),
        "Failed to build project after fetching dependencies"
    );
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn test_wash_wit_wasmcloud_toml_override() {
    let test_setup = load_fixture("integrated-wkg").await.unwrap();

    // Run `wit deps`
    let fetch_status = test_setup
        .base_command()
        .args(["wit", "deps"])
        .status()
        .await
        .expect("Failed to fetch dependencies");
    assert!(fetch_status.success(), "Failed to fetch dependencies");
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn test_wash_wit_wkg_override() {
    let test_setup = load_fixture("separate-wkg").await.unwrap();

    // Run `wit deps`
    let fetch_status = test_setup
        .base_command()
        .args(["wit", "deps"])
        .status()
        .await
        .expect("Failed to fetch dependencies");
    assert!(fetch_status.success(), "Failed to fetch dependencies");
}

/// Succeed on various "extended" configuration pull overrides (valid sources)
#[tokio::test]
async fn test_wash_wit_extended_valid_sources() -> Result<()> {
    let test_setup = load_fixture("integrated-wkg-extended").await.unwrap();

    // Run `wit fetch` (an alias for wit deps)
    let fetch_status = test_setup
        .base_command()
        .args(["wit", "fetch"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status()
        .await
        .expect("Failed to fetch dependencies");
    assert!(fetch_status.success(), "Failed to fetch dependencies");

    // Ensure the expected directories are present
    let wit_output_dir = test_setup.project_dir.join("wit");
    assert!(tokio::fs::metadata(&wit_output_dir)
        .await
        .is_ok_and(|m| m.is_dir()));
    let deps_dir = wit_output_dir.join("deps");
    let wasmcloud_bus_dir = deps_dir.join("wasmcloud-bus-1.0.0");
    let test_components_dir = deps_dir.join("test-components-testing-0.1.0");
    assert!(tokio::fs::metadata(&wasmcloud_bus_dir)
        .await
        .is_ok_and(|m| m.is_dir()));
    assert!(tokio::fs::metadata(&test_components_dir)
        .await
        .is_ok_and(|m| m.is_dir()));
    assert!(tokio::fs::metadata(&wasmcloud_bus_dir)
        .await
        .is_ok_and(|m| m.is_dir()));
    assert!(tokio::fs::metadata(&wasmcloud_bus_dir)
        .await
        .is_ok_and(|m| m.is_dir()));
    let wasmcloud_bus_wit = wasmcloud_bus_dir.join("package.wit");
    let test_components_wit = test_components_dir.join("package.wit");
    assert!(tokio::fs::metadata(&wasmcloud_bus_wit)
        .await
        .is_ok_and(|m| m.is_file()));
    assert!(tokio::fs::metadata(&test_components_wit)
        .await
        .is_ok_and(|m| m.is_file()));

    Ok(())
}

/// Fail on invalid pull source
#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn test_wash_wit_extended_invalid_source() -> Result<()> {
    let test_setup = load_fixture("integrated-wkg-extended").await.unwrap();

    let output = test_setup
        .base_command()
        .args([
            "wit",
            "fetch",
            "--config-path",
            "bad-pull-source.wasmcloud.toml",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .expect("Failed to fetch dependencies");
    assert!(!output.status.success(), "Failed to fetch dependencies");
    Ok(())
}

/// Fail on non-tarball source
#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn test_wash_wit_extended_invalid_tarball() -> Result<()> {
    let test_setup = load_fixture("integrated-wkg-extended").await.unwrap();

    let output = test_setup
        .base_command()
        .args([
            "wit",
            "fetch",
            "--config-path",
            "invalid-tarball.wasmcloud.toml",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .expect("Failed to fetch dependencies");
    assert!(!output.status.success(), "Failed to fetch dependencies");
    Ok(())
}

/// Fail on invalid SHA
#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn test_wash_wit_extended_invalid_sha() -> Result<()> {
    let test_setup = load_fixture("integrated-wkg-extended").await.unwrap();

    let output = test_setup
        .base_command()
        .args([
            "wit",
            "fetch",
            "--config-path",
            "invalid-sha.wasmcloud.toml",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .expect("Failed to fetch dependencies");
    assert!(!output.status.success(), "Failed to fetch dependencies");
    Ok(())
}

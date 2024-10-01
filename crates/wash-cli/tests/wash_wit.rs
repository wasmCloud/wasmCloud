use common::load_fixture;
use tokio::fs;
use wit_component::DecodedWasm;

mod common;

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
async fn test_wash_wit_fetch_and_build() {
    let test_setup = load_fixture("dog-fetcher").await.unwrap();

    // Run `wit fetch`
    let fetch_status = test_setup
        .base_command()
        .args(["wit", "fetch"])
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

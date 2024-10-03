use std::path::PathBuf;

use tokio::process::Command;
use wash_lib::plugin::subcommand::DirMapping;

/// Builds the plugin in the given directory. It must exist inside of tests/plugins/ and the
/// directory name must match the name of the built binary. Returns the path to the built binary.
async fn build_plugin(plugin_dir_name: &str) -> PathBuf {
    let wash_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("Unable to find Cargo dir"));
    // Make sure wash is built
    let status = Command::new("cargo")
        .arg("build")
        .current_dir(wash_dir.join("..").join("wash-cli"))
        .status()
        .await
        .expect("Unable to build wash");
    assert!(status.success(), "Unable to build wash");
    // All of the joins are to avoid path separator issues on windows
    let plugin_dir = wash_dir.join("tests").join("plugins").join(plugin_dir_name);
    // Yes this is hacky and we can change later
    let status = Command::new(
        wash_dir
            .join("..")
            .join("..")
            .join("target")
            .join("debug")
            .join("wash"),
    )
    .arg("build")
    // TODO: Remove this once we've converted everything to the new deps style
    .arg("--skip-fetch")
    .current_dir(&plugin_dir)
    .status()
    .await
    .expect("Unable to build plugin");
    assert!(status.success(), "Unable to build plugin");
    plugin_dir
        .join("build")
        .join(format!("{plugin_dir_name}.wasm"))
}

#[tokio::test]
async fn test_subcommand() {
    let plugin_path = build_plugin("hello_plugin").await;

    let mut subcommand = wash_lib::plugin::subcommand::SubcommandRunner::new().unwrap();
    let metadata = subcommand
        .add_plugin(&plugin_path)
        .await
        .expect("Should be able to add plugin");
    assert_eq!(metadata.name, "Hello Plugin");
    assert_eq!(metadata.version, "0.1.0");
    assert_eq!(metadata.id, "hello");

    let temp = tempfile::tempdir().unwrap();
    let extra_dir = tempfile::tempdir().unwrap();
    tokio::fs::write(extra_dir.path().join("hello.txt"), "hello")
        .await
        .unwrap();
    tokio::fs::write(extra_dir.path().join("world.txt"), "world")
        .await
        .unwrap();

    let file_dir = tempfile::tempdir().unwrap();
    let file = file_dir.path().join("hello.txt");
    tokio::fs::write(&file, "Hello from a file").await.unwrap();

    // TODO: allow configuration of stdout/stderr so we can check for output
    subcommand
        .run(
            "hello",
            temp.path().to_path_buf(),
            vec![
                DirMapping {
                    host_path: extra_dir.path().to_path_buf(),
                    component_path: None,
                },
                DirMapping {
                    host_path: file.clone(),
                    component_path: None,
                },
            ],
            vec![
                "hello".to_string(),
                "--foo".to_string(),
                extra_dir.path().to_str().unwrap().to_string(),
                file.to_str().unwrap().to_string(),
            ],
        )
        .await
        .expect("Should be able to run plugin");

    // Check that the file was written
    let file = tokio::fs::read_to_string(temp.path().join("hello.txt"))
        .await
        .unwrap();
    assert_eq!(file, "Hello from the plugin");
}

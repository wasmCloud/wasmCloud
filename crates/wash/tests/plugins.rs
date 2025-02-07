use std::path::PathBuf;

use wash::lib::plugin::subcommand::DirMapping;

#[tokio::test]
async fn test_subcommand() {
    let mut subcommand = wash::lib::plugin::subcommand::SubcommandRunner::new().unwrap();
    // NOTE: All the joins are to avoid any problems with cross-platform paths
    let plugin_path =
        // This is pre-compiled to save on test time. To rebuild this plugin when changes are needed
        // (assuming relative paths from this file) run:
        // `pushd plugins/hello_plugin && wash build && cp build/hello_plugin_s.wasm ../../fixtures/hello_plugin_s.wasm && popd`
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("Unable to find manifest dir"))
            .join("tests")
            .join("fixtures")
            .join("hello_plugin_s.wasm");
    let metadata = subcommand
        .add_plugin(plugin_path)
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

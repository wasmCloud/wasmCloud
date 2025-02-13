use std::str::FromStr;

use anyhow::Result;
use tempfile::tempdir;
use wash::lib::app::{load_app_manifest, AppManifest, AppManifestSource};

#[tokio::test]
#[cfg_attr(
    not(can_reach_raw_githubusercontent_com),
    ignore = "raw.githubusercontent.com is not reachable"
)]
async fn test_load_app_manifest() -> Result<()> {
    // test stdin
    let stdin = AppManifestSource::AsyncReadSource(Box::new(std::io::Cursor::new(
        "iam batman!".as_bytes(),
    )));

    let manifest = load_app_manifest(stdin).await?;
    assert!(
        matches!(manifest, AppManifest::SerializedModel(manifest) if manifest == "iam batman!"),
        "expected AppManifest::SerializedModel('iam batman!')"
    );

    // create temporary file for this test
    let tmp_dir = tempdir()?;
    tokio::fs::write(tmp_dir.path().join("foo.yaml"), "foo").await?;

    // test file
    let file = AppManifestSource::from_str(tmp_dir.path().join("foo.yaml").to_str().unwrap())?;
    let manifest = load_app_manifest(file).await?;
    assert!(
        matches!(manifest, AppManifest::SerializedModel(manifest) if manifest == "foo"),
        "expected AppManifest::SerializedModel('foo')"
    );

    // test url
    let url = AppManifestSource::from_str(
        "https://raw.githubusercontent.com/wasmCloud/wasmCloud/main/examples/rust/components/http-hello-world/wadm.yaml",
    )?;

    let manifest = load_app_manifest(url).await?;
    assert!(
        matches!(manifest, AppManifest::SerializedModel(_)),
        "expected AppManifest::SerializedModel(_)"
    );

    // test model
    let model = AppManifestSource::from_str("foo")?;
    let manifest = load_app_manifest(model).await?;
    assert!(
        matches!(manifest, AppManifest::ModelName(name) if name == "foo"),
        "expected AppManifest::ModelName('foo')"
    );

    Ok(())
}

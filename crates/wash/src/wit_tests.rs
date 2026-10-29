use super::*;

#[tokio::test]
async fn wit_discovery_prefers_the_root_and_handles_nested_and_empty_sources() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    tokio::fs::create_dir_all(root.join("package/wit")).await?;
    tokio::fs::create_dir(root.join("wit")).await?;
    assert_eq!(find_wit_folder_in_path(root).await?, root.join("wit"));
    tokio::fs::remove_dir(root.join("wit")).await?;
    assert_eq!(
        find_wit_folder_in_path(root).await?,
        root.join("package/wit")
    );
    tokio::fs::remove_dir_all(root.join("package")).await?;
    tokio::fs::write(root.join("package.wit"), "package acme:example;").await?;
    assert_eq!(find_wit_folder_in_path(root).await?, root);
    assert_eq!(find_wit_folder_in_path_internal(root, 11).await?, root);
    assert!(
        find_wit_folder_in_path(&root.join("missing"))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn lock_files_are_created_recovered_and_validated() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join(WKG_LOCK_FILE_NAME);
    let mut lock = load_lock_file(temp.path()).await?;
    lock.write().await?;
    drop(lock);
    let contents = tokio::fs::read_to_string(&path).await?;
    assert!(contents.contains("version = 1"));
    drop(load_lock_file(temp.path()).await?);
    tokio::fs::write(&path, "").await?;
    let mut recovered = load_lock_file(temp.path()).await?;
    recovered.write().await?;
    drop(recovered);
    assert_eq!(tokio::fs::read_to_string(&path).await?, contents);
    for invalid in ["[", "version = 999\npackages = []\n"] {
        tokio::fs::write(&path, invalid).await?;
        assert!(load_lock_file(temp.path()).await.is_err());
        assert_eq!(tokio::fs::read_to_string(&path).await?, invalid);
    }
    Ok(())
}

#[tokio::test]
async fn http_sources_extract_wit_from_a_package_archive() -> Result<()> {
    use std::io::Write as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    wash_runtime::init_crypto();
    let contents = b"package acme:example;";
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    let mut archive = tar::Builder::new(Vec::new());
    archive.append_data(&mut header, "package/wit/example.wit", contents.as_slice())?;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&archive.into_inner()?)?;
    let body = gzip.finish()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut request = [0; 1];
        socket.read_exact(&mut request).await?;
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await?;
        socket.write_all(&body).await?;
        Ok::<_, std::io::Error>(())
    });
    let wit = tokio::time::timeout(
        Duration::from_secs(10),
        download_and_extract_http(&format!("http://{address}/source.tar.gz")),
    )
    .await??;
    server.await??;
    assert_eq!(tokio::fs::read(wit.join("example.wit")).await?, contents);
    let unpacked = wit
        .parent()
        .and_then(Path::parent)
        .context("missing archive root")?;
    assert_eq!(
        unpacked.file_name().and_then(|n| n.to_str()),
        Some("unpacked")
    );
    let temporary = unpacked.parent().context("missing temporary directory")?;
    tokio::fs::remove_dir_all(temporary).await?;
    assert!(download_and_extract_http("invalid URL").await.is_err());
    Ok(())
}

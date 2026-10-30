use super::*;
use bindings::wasi::blobstore::{
    blobstore::Host as Blobstore,
    container::{HostContainer as Container, HostStreamObjectNames as Names},
    types::{HostIncomingValue as Incoming, HostOutgoingValue as Outgoing},
};

fn context(root: &Path) -> SharedCtx {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    let plugin: Arc<dyn HostPlugin + Send + Sync> = Arc::new(FilesystemBlobstore::new(root));
    SharedCtx::new(
        crate::engine::ctx::Ctx::builder("workload", "component")
            .with_plugins(std::collections::HashMap::from([(
                PLUGIN_BLOBSTORE_ID,
                plugin,
            )]))
            .build(),
    )
}

fn borrow<T: 'static>(resource: &Resource<T>) -> Resource<T> {
    Resource::new_borrow(resource.rep())
}

fn object(container: &str, name: &str) -> ObjectId {
    ObjectId {
        container: container.into(),
        object: name.into(),
    }
}

#[tokio::test]
async fn filesystem_values_commit_copy_move_read_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let mut shared = context(dir.path());
    let mut ctx = extract_active_ctx(&mut shared);
    let container = Blobstore::create_container(&mut ctx, "source".into())
        .await
        .unwrap()
        .unwrap();
    assert!(
        Blobstore::container_exists(&mut ctx, "source".into())
            .await
            .unwrap()
            .unwrap()
    );
    assert_eq!(
        Container::name(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .unwrap(),
        "source"
    );
    assert_eq!(
        Container::info(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .unwrap()
            .name,
        "source"
    );
    let reopened = Blobstore::get_container(&mut ctx, "source".into())
        .await
        .unwrap()
        .unwrap();
    Container::drop(&mut ctx, reopened).await.unwrap();

    let outgoing = Outgoing::new_outgoing_value(&mut ctx).await.unwrap();
    let stream = Outgoing::outgoing_value_write_body(&mut ctx, borrow(&outgoing))
        .await
        .unwrap()
        .unwrap();
    ctx.table
        .get_mut(&stream)
        .unwrap()
        .write(bytes::Bytes::from_static(b"abcdef"))
        .unwrap();
    ctx.table.get_mut(&stream).unwrap().flush().unwrap();
    ctx.table.get_mut(&stream).unwrap().ready().await;
    ctx.table.delete(stream).unwrap();
    Container::write_data(
        &mut ctx,
        borrow(&container),
        "nested/data".into(),
        borrow(&outgoing),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!dir.path().join("source/nested/data").exists());
    Outgoing::finish(&mut ctx, outgoing).await.unwrap().unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("source/nested/data")).unwrap(),
        b"abcdef"
    );
    let info = Container::object_info(&mut ctx, borrow(&container), "nested/data".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (info.container.as_str(), info.name.as_str(), info.size),
        ("source", "nested/data", 6)
    );
    assert!(
        Container::has_object(&mut ctx, borrow(&container), "nested/data".into())
            .await
            .unwrap()
            .unwrap()
    );
    for (start, end, expected) in [
        (1, 3, b"bcd".as_slice()),
        (99, 100, b"".as_slice()),
        (0, u64::MAX, b"abcdef".as_slice()),
    ] {
        let incoming = Container::get_data(
            &mut ctx,
            borrow(&container),
            "nested/data".into(),
            start,
            end,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            Incoming::size(&mut ctx, borrow(&incoming)).await.unwrap(),
            expected.len() as u64
        );
        assert_eq!(
            Incoming::incoming_value_consume_sync(&mut ctx, incoming)
                .await
                .unwrap()
                .unwrap(),
            expected
        );
    }
    let incoming = Container::get_data(&mut ctx, borrow(&container), "nested/data".into(), 2, 4)
        .await
        .unwrap()
        .unwrap();
    let stream = Incoming::incoming_value_consume_async(&mut ctx, incoming)
        .await
        .unwrap()
        .unwrap();
    let input = ctx.table.get_mut(&stream).unwrap();
    input.ready().await;
    assert_eq!(input.read(10).unwrap().as_ref(), b"cde");
    ctx.table.delete(stream).unwrap();
    let unused = Container::get_data(&mut ctx, borrow(&container), "nested/data".into(), 0, 0)
        .await
        .unwrap()
        .unwrap();
    Incoming::drop(&mut ctx, unused).await.unwrap();

    Blobstore::copy_object(
        &mut ctx,
        object("source", "nested/data"),
        object("destination", "deep/copied"),
    )
    .await
    .unwrap()
    .unwrap();
    Blobstore::move_object(
        &mut ctx,
        object("destination", "deep/copied"),
        object("destination", "moved/data"),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!dir.path().join("destination/deep/copied").exists());
    assert_eq!(
        std::fs::read(dir.path().join("destination/moved/data")).unwrap(),
        b"abcdef"
    );
    std::fs::write(dir.path().join("source/other"), b"other").unwrap();
    let names = Container::list_objects(&mut ctx, borrow(&container))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Names::skip_stream_object_names(&mut ctx, borrow(&names), 1)
            .await
            .unwrap()
            .unwrap(),
        (1, false)
    );
    let (remaining, done) = Names::read_stream_object_names(&mut ctx, borrow(&names), 5)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(done);
    assert_eq!(
        Names::skip_stream_object_names(&mut ctx, borrow(&names), 2)
            .await
            .unwrap()
            .unwrap(),
        (0, true)
    );
    Names::drop(&mut ctx, names).await.unwrap();
    let names = Container::list_objects(&mut ctx, borrow(&container))
        .await
        .unwrap()
        .unwrap();
    assert!(
        !Names::read_stream_object_names(&mut ctx, borrow(&names), 1)
            .await
            .unwrap()
            .unwrap()
            .1
    );
    Names::drop(&mut ctx, names).await.unwrap();
    Container::delete_object(&mut ctx, borrow(&container), "other".into())
        .await
        .unwrap()
        .unwrap();
    Container::delete_objects(&mut ctx, borrow(&container), vec!["nested/data".into()])
        .await
        .unwrap()
        .unwrap();
    assert!(
        !Container::has_object(&mut ctx, borrow(&container), "other".into())
            .await
            .unwrap()
            .unwrap()
    );
    std::fs::write(dir.path().join("source/clear-me"), b"remove").unwrap();
    Container::clear(&mut ctx, borrow(&container))
        .await
        .unwrap()
        .unwrap();
    assert!(
        Blobstore::container_exists(&mut ctx, "source".into())
            .await
            .unwrap()
            .unwrap()
    );
    assert!(
        std::fs::read_dir(dir.path().join("source"))
            .unwrap()
            .next()
            .is_none()
    );
    std::fs::write(dir.path().join("source/reused"), b"reusable").unwrap();
    assert!(
        Container::has_object(&mut ctx, borrow(&container), "reused".into())
            .await
            .unwrap()
            .unwrap()
    );
    Container::drop(&mut ctx, container).await.unwrap();
    Blobstore::delete_container(&mut ctx, "destination".into())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn filesystem_rejects_escaping_paths_and_reports_missing_objects() {
    let dir = tempfile::tempdir().unwrap();
    let mut shared = context(dir.path());
    let mut ctx = extract_active_ctx(&mut shared);
    let container = Blobstore::create_container(&mut ctx, "valid".into())
        .await
        .unwrap()
        .unwrap();
    assert!(
        Blobstore::create_container(&mut ctx, "../escape".into())
            .await
            .is_err()
    );
    for name in ["../escape", "missing"] {
        assert!(
            Blobstore::get_container(&mut ctx, name.into())
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            Blobstore::delete_container(&mut ctx, name.into())
                .await
                .unwrap()
                .is_err()
        );
    }
    assert!(
        Blobstore::container_exists(&mut ctx, "../escape".into())
            .await
            .unwrap()
            .is_err()
    );
    for name in ["../escape", "missing"] {
        assert!(
            Container::get_data(&mut ctx, borrow(&container), name.into(), 0, 9)
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            Container::object_info(&mut ctx, borrow(&container), name.into())
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            Container::delete_object(&mut ctx, borrow(&container), name.into())
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            Container::delete_objects(&mut ctx, borrow(&container), vec![name.into()])
                .await
                .unwrap()
                .is_err()
        );
    }
    assert!(
        Container::has_object(&mut ctx, borrow(&container), "../escape".into())
            .await
            .unwrap()
            .is_err()
    );
    let outgoing = Outgoing::new_outgoing_value(&mut ctx).await.unwrap();
    assert!(
        Container::write_data(
            &mut ctx,
            borrow(&container),
            "../escape".into(),
            borrow(&outgoing)
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(Outgoing::finish(&mut ctx, outgoing).await.unwrap().is_err());
    let unused = Outgoing::new_outgoing_value(&mut ctx).await.unwrap();
    Outgoing::drop(&mut ctx, unused).await.unwrap();
    for (src, dest) in [
        (object("../escape", "data"), object("valid", "data")),
        (object("valid", "data"), object("../escape", "data")),
        (object("valid", "../escape"), object("valid", "data")),
        (object("valid", "data"), object("valid", "../escape")),
        (object("valid", "missing"), object("valid", "data")),
    ] {
        assert!(
            Blobstore::copy_object(&mut ctx, src.clone(), dest.clone())
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            Blobstore::move_object(&mut ctx, src, dest)
                .await
                .unwrap()
                .is_err()
        );
    }
    std::fs::remove_dir_all(dir.path().join("valid")).unwrap();
    assert!(
        Container::clear(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        Container::list_objects(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        Container::info(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .unwrap()
            .created_at,
        0
    );
    Container::drop(&mut ctx, container).await.unwrap();
}

#[tokio::test]
async fn filesystem_reports_io_failures_and_consumes_failed_handles() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"file").unwrap();
    let mut shared = context(&blocked);
    let mut ctx = extract_active_ctx(&mut shared);
    assert!(
        Blobstore::create_container(&mut ctx, "child".into())
            .await
            .unwrap()
            .is_err()
    );
    let container = ctx
        .table
        .push(ContainerData {
            name: "blocked".into(),
            root: blocked.clone(),
        })
        .unwrap();
    assert!(
        Container::list_objects(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        Blobstore::copy_object(&mut ctx, object("a", "x"), object("b", "nested/x"))
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        Blobstore::move_object(&mut ctx, object("a", "x"), object("b", "nested/x"))
            .await
            .unwrap()
            .is_err()
    );
    let outgoing = Outgoing::new_outgoing_value(&mut ctx).await.unwrap();
    Container::write_data(
        &mut ctx,
        borrow(&container),
        "child/object".into(),
        borrow(&outgoing),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(Outgoing::finish(&mut ctx, outgoing).await.unwrap().is_err());
    for async_read in [false, true] {
        let incoming = ctx
            .table
            .push(IncomingValueHandle {
                file: dir.path().join("absent"),
                start: 0,
                end: 1,
            })
            .unwrap();
        assert!(Incoming::size(&mut ctx, borrow(&incoming)).await.is_err());
        if async_read {
            assert!(
                Incoming::incoming_value_consume_async(&mut ctx, incoming)
                    .await
                    .is_err()
            );
        } else {
            assert!(
                Incoming::incoming_value_consume_sync(&mut ctx, incoming)
                    .await
                    .unwrap()
                    .is_err()
            );
        }
    }
}

#[cfg(any(unix, windows))]
#[tokio::test]
#[cfg_attr(windows, ignore = "requires directory symlink privileges")]
async fn clearing_a_container_removes_links_without_following_them() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"outside data").unwrap();
    let mut shared = context(dir.path());
    let mut ctx = extract_active_ctx(&mut shared);
    let container = Blobstore::create_container(&mut ctx, "source".into())
        .await
        .unwrap()
        .unwrap();
    std::fs::create_dir(dir.path().join("source/nested")).unwrap();
    for name in ["source/link", "source/nested/link"] {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dir.path().join(name)).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, dir.path().join(name)).unwrap();
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.join("keep"), dir.path().join("source/file-link"))
            .unwrap();
        std::os::unix::fs::symlink(outside.join("missing"), dir.path().join("source/dangling"))
            .unwrap();
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(
            outside.join("keep"),
            dir.path().join("source/file-link"),
        )
        .unwrap();
        std::os::windows::fs::symlink_dir(
            outside.join("missing"),
            dir.path().join("source/dangling"),
        )
        .unwrap();
    }
    Container::clear(&mut ctx, borrow(&container))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read(outside.join("keep")).unwrap(),
        b"outside data"
    );
    assert!(dir.path().join("source").is_dir());
    assert!(
        std::fs::read_dir(dir.path().join("source"))
            .unwrap()
            .next()
            .is_none()
    );
    Container::clear(&mut ctx, borrow(&container))
        .await
        .unwrap()
        .unwrap();
    Container::drop(&mut ctx, container).await.unwrap();
}

#[cfg(any(unix, windows))]
#[tokio::test]
#[cfg_attr(windows, ignore = "requires directory symlink privileges")]
async fn clearing_a_symlinked_container_root_preserves_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(outside.path().join("nested")).unwrap();
    std::fs::write(outside.path().join("keep"), b"outside data").unwrap();
    std::fs::write(outside.path().join("nested/keep"), b"nested data").unwrap();
    let source = dir.path().join("source");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &source).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(outside.path(), &source).unwrap();
    let mut shared = context(dir.path());
    let mut ctx = extract_active_ctx(&mut shared);

    for name in ["source", "source/", "source/."] {
        let container = Blobstore::get_container(&mut ctx, name.into())
            .await
            .unwrap()
            .unwrap();
        let error = Container::clear(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.contains("failed to clear container"), "{error}");
        assert_eq!(
            std::fs::read(outside.path().join("keep")).unwrap(),
            b"outside data"
        );
        assert_eq!(
            std::fs::read(outside.path().join("nested/keep")).unwrap(),
            b"nested data"
        );
        assert!(
            std::fs::symlink_metadata(&source)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        Container::drop(&mut ctx, container).await.unwrap();
    }
}

#[cfg(unix)]
#[test]
fn clearing_an_opened_container_does_not_follow_a_replacement_root() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let moved = dir.path().join("moved");
    for root in [source.as_path(), outside.path()] {
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("data"), b"data").unwrap();
        std::fs::write(root.join("nested/data"), b"nested data").unwrap();
    }
    let directory = open_container_directory(&source).unwrap();
    std::fs::rename(&source, &moved).unwrap();
    std::os::unix::fs::symlink(outside.path(), &source).unwrap();

    clear_directory_contents(&directory).unwrap();

    assert!(std::fs::read_dir(&moved).unwrap().next().is_none());
    assert_eq!(std::fs::read(outside.path().join("data")).unwrap(), b"data");
    assert_eq!(
        std::fs::read(outside.path().join("nested/data")).unwrap(),
        b"nested data"
    );
}

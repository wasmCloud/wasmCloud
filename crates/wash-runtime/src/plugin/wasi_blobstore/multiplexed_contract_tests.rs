use super::*;
use bindings::{
    named_imports::wasi::blobstore::blobstore::Host as Blobstore,
    wasi::blobstore::{
        container::{HostContainer as Container, HostStreamObjectNames as Names},
        types::{HostIncomingValue as Incoming, HostOutgoingValue as Outgoing},
    },
};

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
async fn resource_methods_route_to_their_opening_backend() {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    let mut shared =
        SharedCtx::new(crate::engine::ctx::Ctx::builder("workload", "component").build());
    let mut ctx = extract_active_ctx(&mut shared);
    let first: BlobId = Arc::new(InMemoryBackend::default());
    let second: BlobId = Arc::new(InMemoryBackend::default());
    let container = Blobstore::create_container(&mut ctx, first.clone(), "source".into())
        .await
        .unwrap()
        .unwrap();
    assert!(
        Blobstore::create_container(&mut ctx, first.clone(), "source".into())
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        Blobstore::container_exists(&mut ctx, first.clone(), "source".into())
            .await
            .unwrap()
            .unwrap()
    );
    assert!(
        !Blobstore::container_exists(&mut ctx, second.clone(), "source".into())
            .await
            .unwrap()
            .unwrap()
    );
    assert!(
        Blobstore::get_container(&mut ctx, second.clone(), "source".into())
            .await
            .unwrap()
            .is_err()
    );
    let reopened = Blobstore::get_container(&mut ctx, first.clone(), "source".into())
        .await
        .unwrap()
        .unwrap();
    Container::drop(&mut ctx, reopened).await.unwrap();
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
    ctx.table.delete(stream).unwrap();
    Container::write_data(
        &mut ctx,
        borrow(&container),
        "data".into(),
        borrow(&outgoing),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!first.has_object("source", "data").await.unwrap());
    Outgoing::finish(&mut ctx, outgoing).await.unwrap().unwrap();
    assert_eq!(
        first.get_data("source", "data", 0, u64::MAX).await.unwrap(),
        b"abcdef"
    );
    assert!(
        second
            .get_data("source", "data", 0, u64::MAX)
            .await
            .is_err()
    );
    let info = Container::object_info(&mut ctx, borrow(&container), "data".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (info.name.as_str(), info.container.as_str(), info.size),
        ("data", "source", 6)
    );
    let incoming = Container::get_data(&mut ctx, borrow(&container), "data".into(), 1, 3)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Incoming::size(&mut ctx, borrow(&incoming)).await.unwrap(),
        3
    );
    assert_eq!(
        Incoming::incoming_value_consume_sync(&mut ctx, incoming)
            .await
            .unwrap()
            .unwrap(),
        b"bcd"
    );
    let incoming = Container::get_data(&mut ctx, borrow(&container), "data".into(), 0, 0)
        .await
        .unwrap()
        .unwrap();
    let stream = Incoming::incoming_value_consume_async(&mut ctx, incoming)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ctx.table
            .get_mut(&stream)
            .unwrap()
            .read(10)
            .unwrap()
            .as_ref(),
        b"a"
    );
    ctx.table.delete(stream).unwrap();
    let unused = Container::get_data(&mut ctx, borrow(&container), "data".into(), 0, 0)
        .await
        .unwrap()
        .unwrap();
    Incoming::drop(&mut ctx, unused).await.unwrap();
    let unused = Outgoing::new_outgoing_value(&mut ctx).await.unwrap();
    Outgoing::drop(&mut ctx, unused).await.unwrap();
    let destination = Blobstore::create_container(&mut ctx, first.clone(), "destination".into())
        .await
        .unwrap()
        .unwrap();
    Blobstore::copy_object(
        &mut ctx,
        first.clone(),
        object("source", "data"),
        object("destination", "copy"),
    )
    .await
    .unwrap()
    .unwrap();
    Blobstore::move_object(
        &mut ctx,
        first.clone(),
        object("destination", "copy"),
        object("destination", "moved"),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        Container::has_object(&mut ctx, borrow(&destination), "moved".into())
            .await
            .unwrap()
            .unwrap()
    );
    first
        .write_data("destination", "other", b"other".to_vec())
        .await
        .unwrap();
    let names = Container::list_objects(&mut ctx, borrow(&destination))
        .await
        .unwrap()
        .unwrap();
    let (page, end) = Names::read_stream_object_names(&mut ctx, borrow(&names), 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.len(), 1);
    assert!(!end);
    assert_eq!(
        Names::skip_stream_object_names(&mut ctx, borrow(&names), 10)
            .await
            .unwrap()
            .unwrap(),
        (1, true)
    );
    assert_eq!(
        Names::read_stream_object_names(&mut ctx, borrow(&names), 1)
            .await
            .unwrap()
            .unwrap(),
        (vec![], true)
    );
    Names::drop(&mut ctx, names).await.unwrap();
    Container::delete_object(&mut ctx, borrow(&destination), "moved".into())
        .await
        .unwrap()
        .unwrap();
    Container::delete_objects(&mut ctx, borrow(&destination), vec!["other".into()])
        .await
        .unwrap()
        .unwrap();
    Container::clear(&mut ctx, borrow(&container))
        .await
        .unwrap()
        .unwrap();
    assert!(
        !Container::has_object(&mut ctx, borrow(&container), "data".into())
            .await
            .unwrap()
            .unwrap()
    );
    Container::drop(&mut ctx, destination).await.unwrap();
    Blobstore::delete_container(&mut ctx, first.clone(), "destination".into())
        .await
        .unwrap()
        .unwrap();

    first.delete_container("source").await.unwrap();
    assert!(
        Container::info(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        Container::get_data(&mut ctx, borrow(&container), "missing".into(), 0, 0)
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
    assert!(
        Container::object_info(&mut ctx, borrow(&container), "missing".into())
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        Container::delete_object(&mut ctx, borrow(&container), "missing".into())
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        Container::delete_objects(&mut ctx, borrow(&container), vec!["missing".into()])
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        Container::clear(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .is_err()
    );
    assert!(
        Blobstore::delete_container(&mut ctx, first.clone(), "source".into())
            .await
            .unwrap()
            .is_ok()
    );
    assert!(
        Blobstore::copy_object(
            &mut ctx,
            first.clone(),
            object("source", "data"),
            object("destination", "copy")
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(
        Blobstore::move_object(
            &mut ctx,
            first,
            object("source", "data"),
            object("destination", "moved")
        )
        .await
        .unwrap()
        .is_err()
    );
    Container::drop(&mut ctx, container).await.unwrap();
}

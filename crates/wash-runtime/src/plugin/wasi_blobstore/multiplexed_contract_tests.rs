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

fn new_shared() -> SharedCtx {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    SharedCtx::new(crate::engine::ctx::Ctx::builder("workload", "component").build())
}

fn error_message<T>(result: Result<T, BlobError>) -> String {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(message) => message,
    }
}

async fn outgoing_with_body(
    ctx: &mut ActiveCtx<'_>,
    body: &'static [u8],
) -> Resource<OutgoingBlob> {
    let outgoing = Outgoing::new_outgoing_value(ctx).await.unwrap();
    let stream = Outgoing::outgoing_value_write_body(ctx, borrow(&outgoing))
        .await
        .unwrap()
        .unwrap();
    ctx.table
        .get_mut(&stream)
        .unwrap()
        .write(bytes::Bytes::from_static(body))
        .unwrap();
    ctx.table.delete(stream).unwrap();
    outgoing
}

async fn read_object(
    ctx: &mut ActiveCtx<'_>,
    container: &Resource<BlobContainer>,
    name: &str,
) -> Result<Vec<u8>, BlobError> {
    let incoming = match Container::get_data(ctx, borrow(container), name.into(), 0, u64::MAX)
        .await
        .unwrap()
    {
        Ok(incoming) => incoming,
        Err(message) => return Err(message),
    };
    Incoming::incoming_value_consume_sync(ctx, incoming)
        .await
        .unwrap()
}

#[tokio::test]
async fn resource_methods_route_to_their_opening_backend() {
    let mut shared = new_shared();
    let mut ctx = extract_active_ctx(&mut shared);
    let first: BlobId = Arc::new(InMemoryBackend::default());
    let second: BlobId = Arc::new(InMemoryBackend::default());
    let container = Blobstore::create_container(&mut ctx, first.clone(), "source".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        error_message(
            Blobstore::create_container(&mut ctx, first.clone(), "source".into())
                .await
                .unwrap()
        ),
        "container 'source' already exists"
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
    assert_eq!(
        error_message(
            Blobstore::get_container(&mut ctx, second.clone(), "source".into())
                .await
                .unwrap()
        ),
        "container 'source' does not exist"
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
    let outgoing = outgoing_with_body(&mut ctx, b"abcdef").await;
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
    assert!(matches!(
        second.get_data("source", "data", 0, u64::MAX).await,
        Err(BlobBackendError::NoSuchContainer(_))
    ));
    let info = Container::object_info(&mut ctx, borrow(&container), "data".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (info.name.as_str(), info.container.as_str(), info.size),
        ("data", "source", 6)
    );
    assert_eq!(
        error_message(
            Container::object_info(&mut ctx, borrow(&container), "missing".into())
                .await
                .unwrap()
        ),
        "object 'missing' does not exist"
    );
    assert_eq!(
        error_message(read_object(&mut ctx, &container, "missing").await),
        "object 'missing' does not exist"
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
    assert_eq!(
        error_message(
            Blobstore::copy_object(
                &mut ctx,
                first.clone(),
                object("source", "missing"),
                object("destination", "copy"),
            )
            .await
            .unwrap()
        ),
        "object 'missing' does not exist"
    );
    assert_eq!(
        error_message(
            Blobstore::move_object(
                &mut ctx,
                first.clone(),
                object("source", "missing"),
                object("destination", "copy"),
            )
            .await
            .unwrap()
        ),
        "object 'missing' does not exist"
    );
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
    assert_eq!(page, vec!["moved".to_string()]);
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
    assert_eq!(
        first.list_objects("destination").await.unwrap(),
        Vec::<String>::new()
    );
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
    let gone = "container 'source' does not exist";
    assert_eq!(
        Container::name(&mut ctx, borrow(&container))
            .await
            .unwrap()
            .unwrap(),
        "source"
    );
    assert_eq!(
        error_message(Container::info(&mut ctx, borrow(&container)).await.unwrap()),
        gone
    );
    assert_eq!(
        error_message(read_object(&mut ctx, &container, "missing").await),
        gone
    );
    assert_eq!(
        error_message(
            Container::list_objects(&mut ctx, borrow(&container))
                .await
                .unwrap()
        ),
        gone
    );
    assert_eq!(
        error_message(
            Container::object_info(&mut ctx, borrow(&container), "missing".into())
                .await
                .unwrap()
        ),
        gone
    );
    assert_eq!(
        error_message(
            Container::has_object(&mut ctx, borrow(&container), "missing".into())
                .await
                .unwrap()
        ),
        gone
    );
    assert_eq!(
        error_message(
            Container::delete_object(&mut ctx, borrow(&container), "missing".into())
                .await
                .unwrap()
        ),
        gone
    );
    assert_eq!(
        error_message(
            Container::delete_objects(&mut ctx, borrow(&container), vec!["missing".into()])
                .await
                .unwrap()
        ),
        gone
    );
    assert_eq!(
        error_message(
            Container::clear(&mut ctx, borrow(&container))
                .await
                .unwrap()
        ),
        gone
    );
    let late = outgoing_with_body(&mut ctx, b"late").await;
    Container::write_data(&mut ctx, borrow(&container), "data".into(), borrow(&late))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        error_message(Outgoing::finish(&mut ctx, late).await.unwrap()),
        gone
    );
    assert!(
        Blobstore::delete_container(&mut ctx, first.clone(), "source".into())
            .await
            .unwrap()
            .is_ok()
    );
    assert_eq!(
        error_message(
            Blobstore::copy_object(
                &mut ctx,
                first.clone(),
                object("source", "data"),
                object("destination", "copy"),
            )
            .await
            .unwrap()
        ),
        gone
    );
    assert_eq!(
        error_message(
            Blobstore::move_object(
                &mut ctx,
                first,
                object("source", "data"),
                object("destination", "moved"),
            )
            .await
            .unwrap()
        ),
        gone
    );
    Container::drop(&mut ctx, container).await.unwrap();
}

#[tokio::test]
async fn container_resources_route_by_opening_backend_when_names_collide() {
    let mut shared = new_shared();
    let mut ctx = extract_active_ctx(&mut shared);
    let first: BlobId = Arc::new(InMemoryBackend::default());
    let second: BlobId = Arc::new(InMemoryBackend::default());
    for (backend, body) in [
        (&first, b"first".as_slice()),
        (&second, b"second".as_slice()),
    ] {
        backend.create_container("source").await.unwrap();
        backend
            .write_data("source", "k", body.to_vec())
            .await
            .unwrap();
    }
    let via_first = Blobstore::get_container(&mut ctx, first.clone(), "source".into())
        .await
        .unwrap()
        .unwrap();
    let via_second = Blobstore::get_container(&mut ctx, second.clone(), "source".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        read_object(&mut ctx, &via_first, "k").await.unwrap(),
        b"first"
    );
    assert_eq!(
        read_object(&mut ctx, &via_second, "k").await.unwrap(),
        b"second"
    );

    Container::delete_object(&mut ctx, borrow(&via_first), "k".into())
        .await
        .unwrap()
        .unwrap();
    assert!(!first.has_object("source", "k").await.unwrap());
    assert!(second.has_object("source", "k").await.unwrap());
    assert_eq!(
        read_object(&mut ctx, &via_second, "k").await.unwrap(),
        b"second"
    );

    Blobstore::copy_object(
        &mut ctx,
        second.clone(),
        object("source", "k"),
        object("source", "copy"),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(second.has_object("source", "copy").await.unwrap());
    assert!(!first.has_object("source", "copy").await.unwrap());
    assert_eq!(
        error_message(read_object(&mut ctx, &via_first, "copy").await),
        "object 'copy' does not exist"
    );
    Container::drop(&mut ctx, via_first).await.unwrap();
    Container::drop(&mut ctx, via_second).await.unwrap();
}

#[tokio::test]
async fn finish_reports_a_vanished_destination_and_drop_flushes_pending_writes() {
    let mut shared = new_shared();
    let mut ctx = extract_active_ctx(&mut shared);
    let backend: BlobId = Arc::new(InMemoryBackend::default());

    let vanishing = Blobstore::create_container(&mut ctx, backend.clone(), "vanishing".into())
        .await
        .unwrap()
        .unwrap();
    let outgoing = outgoing_with_body(&mut ctx, b"late").await;
    Container::write_data(
        &mut ctx,
        borrow(&vanishing),
        "data".into(),
        borrow(&outgoing),
    )
    .await
    .unwrap()
    .unwrap();
    backend.delete_container("vanishing").await.unwrap();
    assert_eq!(
        error_message(Outgoing::finish(&mut ctx, outgoing).await.unwrap()),
        "container 'vanishing' does not exist"
    );

    let kept = Blobstore::create_container(&mut ctx, backend.clone(), "kept".into())
        .await
        .unwrap()
        .unwrap();
    let outgoing = outgoing_with_body(&mut ctx, b"flushed").await;
    Container::write_data(&mut ctx, borrow(&kept), "data".into(), borrow(&outgoing))
        .await
        .unwrap()
        .unwrap();
    assert!(!backend.has_object("kept", "data").await.unwrap());
    Outgoing::drop(&mut ctx, outgoing).await.unwrap();
    assert_eq!(
        backend.get_data("kept", "data", 0, u64::MAX).await.unwrap(),
        b"flushed"
    );

    let first = Blobstore::create_container(&mut ctx, backend.clone(), "first".into())
        .await
        .unwrap()
        .unwrap();
    let second = Blobstore::create_container(&mut ctx, backend.clone(), "second".into())
        .await
        .unwrap()
        .unwrap();
    let outgoing = outgoing_with_body(&mut ctx, b"once").await;
    Container::write_data(&mut ctx, borrow(&first), "data".into(), borrow(&outgoing))
        .await
        .unwrap()
        .unwrap();
    Container::write_data(
        &mut ctx,
        borrow(&second),
        "retargeted".into(),
        borrow(&outgoing),
    )
    .await
    .unwrap()
    .unwrap();
    Outgoing::finish(&mut ctx, outgoing).await.unwrap().unwrap();
    assert_eq!(
        backend.list_objects("first").await.unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        backend.list_objects("second").await.unwrap(),
        vec!["retargeted".to_string()]
    );
    assert_eq!(
        backend
            .get_data("second", "retargeted", 0, u64::MAX)
            .await
            .unwrap(),
        b"once"
    );

    for container in [vanishing, kept, first, second] {
        Container::drop(&mut ctx, container).await.unwrap();
    }
}

#[test]
fn clamp_range_clamps_both_ends() {
    let cases = [
        ((0, u64::MAX, 5), 0..5),
        ((2, 1, 5), 2..2),
        ((7, 9, 5), 5..5),
        ((1, 3, 5), 1..4),
        ((0, 0, 0), 0..0),
        ((0, 0, 5), 0..1),
        ((4, u64::MAX, 5), 4..5),
    ];
    for ((start, end, len), expected) in cases {
        assert_eq!(
            clamp_range(start, end, len),
            expected,
            "clamp_range({start}, {end}, {len})"
        );
    }
}

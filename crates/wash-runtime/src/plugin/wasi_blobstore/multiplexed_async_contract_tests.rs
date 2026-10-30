use super::super::multiplexed::InMemoryBackend;
use super::*;
use crate::engine::ctx::{Ctx, CtxBuilder};
use crate::plugin::stream_collect::collect_stream;
use bindings::{
    named_imports::wasmcloud::blobstore::blobstore::HostWithStore as Named,
    wasmcloud::blobstore::{
        blobstore::HostWithStore as Plain,
        container::{HostContainer, HostContainerWithStore as Container},
    },
};
use futures::FutureExt as _;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};
use wasmtime::component::{Source, StreamConsumer, StreamResult};

type HostAccessor = Accessor<SharedCtx, SharedCtx>;

fn borrow<T: 'static>(resource: &Resource<T>) -> Resource<T> {
    Resource::new_borrow(resource.rep())
}

fn object(container: &str, name: &str) -> ObjectId {
    ObjectId {
        container: container.into(),
        object: name.into(),
    }
}

fn ctx_builder() -> CtxBuilder {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    Ctx::builder("workload", "component")
}

fn ctx_with_plugin(plugin: MultiplexedAsyncBlobstore) -> Ctx {
    let plugin: Arc<dyn HostPlugin + Send + Sync> = Arc::new(plugin);
    ctx_builder()
        .with_plugins(HashMap::from([(MULTIPLEXED_ASYNC_BLOBSTORE_ID, plugin)]))
        .build()
}

fn ctx_with_default(backend: &BlobId) -> Ctx {
    let plugin = MultiplexedAsyncBlobstore::new();
    plugin
        .mux
        .set_default("workload", &HashMap::from([("".into(), backend.clone())]));
    ctx_with_plugin(plugin)
}

async fn with_host<R>(ctx: Ctx, body: impl AsyncFnOnce(&HostAccessor) -> R) -> R {
    let mut config = wasmtime::Config::new();
    config
        .wasm_component_model_async(true)
        .concurrency_support(true);
    let engine = wasmtime::Engine::new(&config).unwrap();
    let mut store = wasmtime::Store::new(&engine, SharedCtx::new(ctx));
    let run = store.run_concurrent(async move |accessor| {
        let host = accessor.with_getter::<SharedCtx>(extract_active_ctx);
        body(&host).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), run)
        .await
        .unwrap()
        .unwrap()
}

enum Route {
    Plain,
    Named(BlobId),
}

impl Route {
    async fn create_container(
        &self,
        host: &HostAccessor,
        name: &str,
    ) -> Result<Resource<BlobContainer>, AsyncError> {
        match self {
            Route::Plain => <SharedCtx as Plain<_>>::create_container(host, name.into())
                .await
                .unwrap(),
            Route::Named(id) => {
                <SharedCtx as Named<_>>::create_container(host, id.clone(), name.into())
                    .await
                    .unwrap()
            }
        }
    }

    async fn get_container(
        &self,
        host: &HostAccessor,
        name: &str,
    ) -> Result<Resource<BlobContainer>, AsyncError> {
        match self {
            Route::Plain => <SharedCtx as Plain<_>>::get_container(host, name.into())
                .await
                .unwrap(),
            Route::Named(id) => {
                <SharedCtx as Named<_>>::get_container(host, id.clone(), name.into())
                    .await
                    .unwrap()
            }
        }
    }

    async fn delete_container(&self, host: &HostAccessor, name: &str) -> Result<(), AsyncError> {
        match self {
            Route::Plain => <SharedCtx as Plain<_>>::delete_container(host, name.into())
                .await
                .unwrap(),
            Route::Named(id) => {
                <SharedCtx as Named<_>>::delete_container(host, id.clone(), name.into())
                    .await
                    .unwrap()
            }
        }
    }

    async fn container_exists(&self, host: &HostAccessor, name: &str) -> Result<bool, AsyncError> {
        match self {
            Route::Plain => <SharedCtx as Plain<_>>::container_exists(host, name.into())
                .await
                .unwrap(),
            Route::Named(id) => {
                <SharedCtx as Named<_>>::container_exists(host, id.clone(), name.into())
                    .await
                    .unwrap()
            }
        }
    }

    async fn copy_object(
        &self,
        host: &HostAccessor,
        src: ObjectId,
        dest: ObjectId,
    ) -> Result<(), AsyncError> {
        match self {
            Route::Plain => <SharedCtx as Plain<_>>::copy_object(host, src, dest)
                .await
                .unwrap(),
            Route::Named(id) => <SharedCtx as Named<_>>::copy_object(host, id.clone(), src, dest)
                .await
                .unwrap(),
        }
    }

    async fn move_object(
        &self,
        host: &HostAccessor,
        src: ObjectId,
        dest: ObjectId,
    ) -> Result<(), AsyncError> {
        match self {
            Route::Plain => <SharedCtx as Plain<_>>::move_object(host, src, dest)
                .await
                .unwrap(),
            Route::Named(id) => <SharedCtx as Named<_>>::move_object(host, id.clone(), src, dest)
                .await
                .unwrap(),
        }
    }
}

async fn container_name(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
) -> Result<String, AsyncError> {
    <SharedCtx as Container<_>>::name(host, borrow(container))
        .await
        .unwrap()
}

async fn container_info(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
) -> Result<ContainerMetadata, AsyncError> {
    <SharedCtx as Container<_>>::info(host, borrow(container))
        .await
        .unwrap()
}

async fn read_range(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
    name: &str,
    start: u64,
    end: u64,
) -> Result<Vec<u8>, AsyncError> {
    let result =
        <SharedCtx as Container<_>>::get_data(host, borrow(container), name.into(), start, end)
            .await
            .unwrap();
    match result {
        Ok(body) => Ok(collect_stream(host, body, usize::MAX)
            .await
            .unwrap()
            .unwrap()),
        Err(e) => Err(e),
    }
}

async fn read_all(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
    name: &str,
) -> Result<Vec<u8>, AsyncError> {
    read_range(host, container, name, 0, u64::MAX).await
}

async fn write_object(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
    name: &str,
    body: &[u8],
) -> Result<(), AsyncError> {
    let body = host
        .with(|mut access| StreamReader::new(&mut access, body.to_vec()))
        .unwrap();
    <SharedCtx as Container<_>>::write_data(host, borrow(container), name.into(), body)
        .await
        .unwrap()
}

async fn list_objects(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
) -> Result<StreamReader<ObjectName>, AsyncError> {
    <SharedCtx as Container<_>>::list_objects(host, borrow(container))
        .await
        .unwrap()
}

async fn object_info(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
    name: &str,
) -> Result<ObjectMetadata, AsyncError> {
    <SharedCtx as Container<_>>::object_info(host, borrow(container), name.into())
        .await
        .unwrap()
}

async fn has_object(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
    name: &str,
) -> Result<bool, AsyncError> {
    <SharedCtx as Container<_>>::has_object(host, borrow(container), name.into())
        .await
        .unwrap()
}

async fn delete_object(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
    name: &str,
) -> Result<(), AsyncError> {
    <SharedCtx as Container<_>>::delete_object(host, borrow(container), name.into())
        .await
        .unwrap()
}

async fn delete_objects(
    host: &HostAccessor,
    container: &Resource<BlobContainer>,
    names: Vec<ObjectName>,
) -> Result<(), AsyncError> {
    <SharedCtx as Container<_>>::delete_objects(host, borrow(container), names)
        .await
        .unwrap()
}

async fn clear(host: &HostAccessor, container: &Resource<BlobContainer>) -> Result<(), AsyncError> {
    <SharedCtx as Container<_>>::clear(host, borrow(container))
        .await
        .unwrap()
}

fn drop_container(host: &HostAccessor, container: Resource<BlobContainer>) {
    host.with(|mut access| {
        let mut ctx = access.get();
        <ActiveCtx as HostContainer>::drop(&mut ctx, container)
            .now_or_never()
            .expect("dropping a container never suspends")
            .unwrap();
    });
}

struct NameCollector {
    names: Vec<ObjectName>,
    done: Option<tokio::sync::oneshot::Sender<Vec<ObjectName>>>,
}

impl Drop for NameCollector {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.send(std::mem::take(&mut self.names));
        }
    }
}

impl StreamConsumer<SharedCtx> for NameCollector {
    type Item = ObjectName;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: wasmtime::StoreContextMut<'_, SharedCtx>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        let available = source.remaining(&mut store);
        if available == 0 {
            return Poll::Ready(Ok(if finish {
                StreamResult::Cancelled
            } else {
                StreamResult::Completed
            }));
        }
        this.names.reserve(available);
        match source.read(&mut store, &mut this.names) {
            Ok(()) => Poll::Ready(Ok(StreamResult::Completed)),
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

async fn read_listing(host: &HostAccessor, container: &Resource<BlobContainer>) -> Vec<ObjectName> {
    let names = list_objects(host, container).await.unwrap();
    let (done, finished) = tokio::sync::oneshot::channel();
    let collector = NameCollector {
        names: Vec::new(),
        done: Some(done),
    };
    host.with(|mut access| names.pipe(&mut access, collector))
        .unwrap();
    finished.await.unwrap()
}

fn is_no_default<T>(result: &Result<T, AsyncError>) -> bool {
    matches!(
        result,
        Err(AsyncError::Other(message))
            if message == "no default wasmcloud:blobstore backend is bound for this component"
    )
}

fn trap_message<T>(result: wasmtime::Result<T>) -> String {
    match result {
        Ok(_) => panic!("expected a trap"),
        Err(trap) => trap.to_string(),
    }
}

async fn exercise_routed_lifecycle(host: &HostAccessor, route: &Route, backend: &BlobId) {
    assert!(!route.container_exists(host, "source").await.unwrap());
    assert!(matches!(
        route.get_container(host, "source").await,
        Err(AsyncError::NoSuchContainer)
    ));
    let container = route.create_container(host, "source").await.unwrap();
    assert!(matches!(
        route.create_container(host, "source").await,
        Err(AsyncError::ContainerAlreadyExists)
    ));
    assert!(route.container_exists(host, "source").await.unwrap());
    let reopened = route.get_container(host, "source").await.unwrap();
    drop_container(host, reopened);

    assert_eq!(container_name(host, &container).await.unwrap(), "source");
    assert_eq!(
        container_info(host, &container).await.unwrap().name,
        "source"
    );
    write_object(host, &container, "data", b"abcdef")
        .await
        .unwrap();
    assert_eq!(
        backend
            .get_data("source", "data", 0, u64::MAX)
            .await
            .unwrap(),
        b"abcdef"
    );
    assert_eq!(
        read_range(host, &container, "data", 1, 3).await.unwrap(),
        b"bcd"
    );
    assert_eq!(object_info(host, &container, "data").await.unwrap().size, 6);

    let destination = route.create_container(host, "destination").await.unwrap();
    assert!(matches!(
        object_info(host, &container, "missing").await,
        Err(AsyncError::NoSuchObject)
    ));
    assert!(matches!(
        read_all(host, &container, "missing").await,
        Err(AsyncError::NoSuchObject)
    ));
    assert!(matches!(
        route
            .copy_object(
                host,
                object("source", "missing"),
                object("destination", "copy")
            )
            .await,
        Err(AsyncError::NoSuchObject)
    ));
    assert!(matches!(
        route
            .move_object(
                host,
                object("source", "missing"),
                object("destination", "copy")
            )
            .await,
        Err(AsyncError::NoSuchObject)
    ));

    route
        .copy_object(
            host,
            object("source", "data"),
            object("destination", "copy"),
        )
        .await
        .unwrap();
    route
        .move_object(
            host,
            object("destination", "copy"),
            object("destination", "moved"),
        )
        .await
        .unwrap();
    assert!(has_object(host, &destination, "moved").await.unwrap());
    assert!(!has_object(host, &destination, "copy").await.unwrap());
    delete_object(host, &destination, "moved").await.unwrap();
    backend
        .write_data("destination", "other", vec![1])
        .await
        .unwrap();
    delete_objects(host, &destination, vec!["other".into()])
        .await
        .unwrap();
    assert!(!has_object(host, &destination, "other").await.unwrap());
    clear(host, &container).await.unwrap();
    assert!(!has_object(host, &container, "data").await.unwrap());

    route.delete_container(host, "source").await.unwrap();
    assert_eq!(container_name(host, &container).await.unwrap(), "source");
    assert!(matches!(
        container_info(host, &container).await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        read_range(host, &container, "data", 0, 1).await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        write_object(host, &container, "data", b"late").await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        list_objects(host, &container).await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        object_info(host, &container, "data").await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        has_object(host, &container, "data").await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        delete_object(host, &container, "data").await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        delete_objects(host, &container, vec!["data".into()]).await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        clear(host, &container).await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        route
            .copy_object(
                host,
                object("source", "missing"),
                object("destination", "copy")
            )
            .await,
        Err(AsyncError::NoSuchContainer)
    ));
    assert!(matches!(
        route
            .move_object(
                host,
                object("source", "missing"),
                object("destination", "copy")
            )
            .await,
        Err(AsyncError::NoSuchContainer)
    ));
    drop_container(host, container);
    drop_container(host, destination);
}

#[tokio::test]
async fn async_routes_keep_resources_and_streams_on_the_selected_backend() {
    for named in [false, true] {
        let backend: BlobId = Arc::new(InMemoryBackend::new());
        let route = if named {
            Route::Named(backend.clone())
        } else {
            Route::Plain
        };
        let ctx = ctx_with_default(&backend);
        with_host(ctx, async |host| {
            exercise_routed_lifecycle(host, &route, &backend).await
        })
        .await;
    }
}

async fn exercise_colliding_names(host: &HostAccessor, backend_a: &BlobId, backend_b: &BlobId) {
    let named = Route::Named(backend_b.clone());
    let cb = named.create_container(host, "source").await.unwrap();
    let ca = Route::Plain.create_container(host, "source").await.unwrap();
    write_object(host, &cb, "k", b"B").await.unwrap();
    write_object(host, &ca, "k", b"A").await.unwrap();

    assert_eq!(read_all(host, &cb, "k").await.unwrap(), b"B");
    assert_eq!(read_all(host, &ca, "k").await.unwrap(), b"A");
    assert_eq!(
        backend_b
            .get_data("source", "k", 0, u64::MAX)
            .await
            .unwrap(),
        b"B"
    );
    assert_eq!(
        backend_a
            .get_data("source", "k", 0, u64::MAX)
            .await
            .unwrap(),
        b"A"
    );

    Route::Plain.delete_container(host, "source").await.unwrap();
    assert_eq!(container_info(host, &cb).await.unwrap().name, "source");
    assert!(matches!(
        container_info(host, &ca).await,
        Err(AsyncError::NoSuchContainer)
    ));
    drop_container(host, ca);
    drop_container(host, cb);
}

#[tokio::test]
async fn async_container_methods_follow_the_opening_backend_not_the_default() {
    let backend_a: BlobId = Arc::new(InMemoryBackend::new());
    let backend_b: BlobId = Arc::new(InMemoryBackend::new());
    let ctx = ctx_with_default(&backend_a);
    with_host(ctx, async |host| {
        exercise_colliding_names(host, &backend_a, &backend_b).await
    })
    .await;
}

async fn exercise_listing(host: &HostAccessor) {
    let container = Route::Plain.create_container(host, "source").await.unwrap();
    for name in ["b", "a", "c"] {
        write_object(host, &container, name, b"x").await.unwrap();
    }
    assert_eq!(read_listing(host, &container).await, ["a", "b", "c"]);

    let empty = Route::Plain.create_container(host, "empty").await.unwrap();
    assert_eq!(read_listing(host, &empty).await, Vec::<String>::new());
    drop_container(host, container);
    drop_container(host, empty);
}

#[tokio::test]
async fn async_list_objects_streams_the_backend_listing() {
    let backend: BlobId = Arc::new(InMemoryBackend::new());
    let ctx = ctx_with_default(&backend);
    with_host(ctx, async |host| exercise_listing(host).await).await;
}

async fn exercise_missing_default(host: &HostAccessor) {
    let route = Route::Plain;
    assert!(is_no_default(&route.create_container(host, "data").await));
    assert!(is_no_default(&route.get_container(host, "data").await));
    assert!(is_no_default(&route.delete_container(host, "data").await));
    assert!(is_no_default(&route.container_exists(host, "data").await));
    assert!(is_no_default(
        &route
            .copy_object(host, object("data", "a"), object("data", "b"))
            .await
    ));
    assert!(is_no_default(
        &route
            .move_object(host, object("data", "a"), object("data", "b"))
            .await
    ));
}

#[tokio::test]
async fn plain_calls_without_a_default_report_configuration_errors() {
    let ctx = ctx_with_plugin(MultiplexedAsyncBlobstore::new());
    with_host(ctx, async |host| exercise_missing_default(host).await).await;
}

async fn plain_call_traps(host: &HostAccessor) -> [String; 6] {
    [
        trap_message(<SharedCtx as Plain<_>>::create_container(host, "data".into()).await),
        trap_message(<SharedCtx as Plain<_>>::get_container(host, "data".into()).await),
        trap_message(<SharedCtx as Plain<_>>::delete_container(host, "data".into()).await),
        trap_message(<SharedCtx as Plain<_>>::container_exists(host, "data".into()).await),
        trap_message(
            <SharedCtx as Plain<_>>::copy_object(host, object("data", "a"), object("data", "b"))
                .await,
        ),
        trap_message(
            <SharedCtx as Plain<_>>::move_object(host, object("data", "a"), object("data", "b"))
                .await,
        ),
    ]
}

#[tokio::test]
async fn plain_calls_trap_when_the_plugin_is_absent() {
    let ctx = ctx_builder().build();
    let traps = with_host(ctx, async |host| plain_call_traps(host).await).await;
    let expected = format!("plugin {MULTIPLEXED_ASYNC_BLOBSTORE_ID} not found");
    for trap in traps {
        assert!(trap.contains(&expected), "unexpected trap: {trap}");
    }
}

//! The plain (unlabeled) route.
//!
//! A component that imports `wasmcloud:nats` without an `(implements ..)` label
//! names no binding, so its calls go out on the workload's unnamed binding —
//! the only shape that existed before named bindings, and still the common one.
//! Each of these resolves that connection and hands it to the label-routed
//! implementation, so the two routes cannot drift.

use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::SharedCtx;

use super::{conn_or_return, core, js, kv, labeled_core, labeled_js, labeled_kv, types};
use crate::plugin::wasmcloud_nats::handles::{BucketHandle, PullConsumerHandle};

impl<T: 'static + Send> core::HostWithStore<T> for SharedCtx {
    async fn publish(
        accessor: &Accessor<T, Self>,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_core::HostWithStore<T>>::publish(accessor, conn, msg).await
    }
    async fn request(
        accessor: &Accessor<T, Self>,
        msg: types::NatsMessage,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<types::NatsMessage, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_core::HostWithStore<T>>::request(accessor, conn, msg, timeout_ms).await
    }
}

impl<T: 'static + Send> js::HostWithStore<T> for SharedCtx {
    async fn publish(
        accessor: &Accessor<T, Self>,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<js::PublishAck, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::publish(accessor, conn, msg).await
    }
    async fn get_by_sequence(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        sequence: u64,
    ) -> wasmtime::Result<Result<js::StoredMessage, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::get_by_sequence(
            accessor,
            conn,
            stream_name,
            sequence,
        )
        .await
    }
    async fn scan(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        start_sequence: u64,
        max_count: u32,
    ) -> wasmtime::Result<Result<Vec<js::StoredMessage>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::scan(
            accessor,
            conn,
            stream_name,
            start_sequence,
            max_count,
        )
        .await
    }
    async fn open_pull_consumer(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<Resource<PullConsumerHandle>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::open_pull_consumer(
            accessor,
            conn,
            stream_name,
            consumer,
        )
        .await
    }
    async fn get_stream_info(
        accessor: &Accessor<T, Self>,
        stream_name: String,
    ) -> wasmtime::Result<Result<js::StreamInfo, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::get_stream_info(accessor, conn, stream_name).await
    }
    async fn list_stream_subjects(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        subject_filter: String,
    ) -> wasmtime::Result<Result<Vec<js::SubjectCount>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::list_stream_subjects(
            accessor,
            conn,
            stream_name,
            subject_filter,
        )
        .await
    }
    async fn get_consumer_info(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<js::ConsumerInfo, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::get_consumer_info(
            accessor,
            conn,
            stream_name,
            consumer,
        )
        .await
    }
}

impl<T: 'static + Send> kv::HostWithStore<T> for SharedCtx {
    async fn open(
        accessor: &Accessor<T, Self>,
        bucket: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_kv::HostWithStore<T>>::open(accessor, conn, bucket).await
    }
}

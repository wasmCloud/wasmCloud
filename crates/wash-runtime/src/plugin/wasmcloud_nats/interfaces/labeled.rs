//! Label-routed resource methods.
//!
//! A message handle, pull consumer, or bucket already carries the connection it
//! was opened through — an `Acker`, a `Consumer`, a `Store` — so its methods
//! need no routing and ignore the label. They exist only because the resources
//! live in routed interfaces, and delegate to the plain implementations.

use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::{ActiveCtx, SharedCtx};

use super::{NatsId, js, kv, labeled_core, labeled_js, labeled_kv, types};
use crate::plugin::wasmcloud_nats::jetstream::{BucketHandle, MessageHandle, PullConsumerHandle};

impl<T: 'static + Send> labeled_js::HostMessageHandleWithStore<T> for SharedCtx {
    async fn ack(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::ack(accessor, rep).await
    }
    async fn ack_sync(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::ack_sync(accessor, rep).await
    }
    async fn nak(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
        delay_ms: Option<u32>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::nak(accessor, rep, delay_ms).await
    }
    async fn term(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::term(accessor, rep).await
    }
    async fn in_progress(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::in_progress(accessor, rep).await
    }
}

impl labeled_js::HostMessageHandle for ActiveCtx<'_> {
    async fn message(
        &mut self,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<types::NatsMessage> {
        <Self as js::HostMessageHandle>::message(self, rep).await
    }
    async fn sequence(
        &mut self,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<u64> {
        <Self as js::HostMessageHandle>::sequence(self, rep).await
    }
    async fn delivery_count(
        &mut self,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<u32> {
        <Self as js::HostMessageHandle>::delivery_count(self, rep).await
    }
    async fn drop(&mut self, _id: NatsId, rep: Resource<MessageHandle>) -> wasmtime::Result<()> {
        <Self as js::HostMessageHandle>::drop(self, rep).await
    }
}

impl<T: 'static + Send> labeled_js::HostPullConsumerWithStore<T> for SharedCtx {
    async fn fetch(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<PullConsumerHandle>,
        batch: u32,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<js::FetchedBatch, types::NatsError>> {
        <Self as js::HostPullConsumerWithStore<T>>::fetch(accessor, rep, batch, timeout_ms).await
    }
    async fn fetch_with_limits(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<PullConsumerHandle>,
        batch: u32,
        max_bytes: u64,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<js::FetchedBatch, types::NatsError>> {
        <Self as js::HostPullConsumerWithStore<T>>::fetch_with_limits(
            accessor, rep, batch, max_bytes, timeout_ms,
        )
        .await
    }
    async fn info(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<PullConsumerHandle>,
    ) -> wasmtime::Result<Result<js::ConsumerInfo, types::NatsError>> {
        <Self as js::HostPullConsumerWithStore<T>>::info(accessor, rep).await
    }
}

impl labeled_js::HostPullConsumer for ActiveCtx<'_> {
    async fn drop(
        &mut self,
        _id: NatsId,
        rep: Resource<PullConsumerHandle>,
    ) -> wasmtime::Result<()> {
        <Self as js::HostPullConsumer>::drop(self, rep).await
    }
}

impl<T: 'static + Send> labeled_kv::HostBucketWithStore<T> for SharedCtx {
    async fn get(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<kv::Entry, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::get(accessor, rep, key).await
    }
    async fn put(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::put(accessor, rep, key, value).await
    }
    async fn create(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::create(accessor, rep, key, value).await
    }
    async fn update(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
        expected_revision: u64,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::update(accessor, rep, key, value, expected_revision)
            .await
    }
    async fn delete(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::delete(accessor, rep, key).await
    }
    async fn purge(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::purge(accessor, rep, key).await
    }
    async fn keys(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        filter: String,
    ) -> wasmtime::Result<Result<kv::KeyPage, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::keys(accessor, rep, filter).await
    }
    async fn history(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Vec<kv::Entry>, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::history(accessor, rep, key).await
    }
    async fn status(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
    ) -> wasmtime::Result<Result<kv::BucketStatus, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::status(accessor, rep).await
    }
}

impl labeled_kv::HostBucket for ActiveCtx<'_> {
    async fn drop(&mut self, _id: NatsId, rep: Resource<BucketHandle>) -> wasmtime::Result<()> {
        <Self as kv::HostBucket>::drop(self, rep).await
    }
}

// Marker traits: the interfaces carry no free-standing host state.
impl labeled_core::Host for ActiveCtx<'_> {}
impl labeled_js::Host for ActiveCtx<'_> {}
impl labeled_kv::Host for ActiveCtx<'_> {}

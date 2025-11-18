use super::{new_store, Ctx, Handler, Instance, ReplacedInstanceTarget};

use crate::capability::keyvalue::{atomics, batch, store};
use crate::capability::wrpc;

use anyhow::Context;
use bytes::Bytes;
use std::sync::Arc;
use tracing::{debug, instrument, trace};
use wasmtime::component::Resource;

type Result<T, E = store::Error> = core::result::Result<T, E>;

pub mod keyvalue_watcher_bindings {
    wasmtime::component::bindgen!({
        world: "watcher",
        imports: { default: async | trappable | tracing },
        exports: { default: async | trappable | tracing },
        with: {
            "wasi:keyvalue/store" : crate::capability::keyvalue::store,
        }
    });
}

impl From<wrpc::wrpc::keyvalue::store::Error> for store::Error {
    fn from(value: wrpc::wrpc::keyvalue::store::Error) -> Self {
        match value {
            wrpc::wrpc::keyvalue::store::Error::NoSuchStore => Self::NoSuchStore,
            wrpc::wrpc::keyvalue::store::Error::AccessDenied => Self::AccessDenied,
            wrpc::wrpc::keyvalue::store::Error::Other(other) => Self::Other(other),
        }
    }
}

impl<H> atomics::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument(level = "debug", skip_all)]
    async fn increment(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64>> {
        self.attach_parent_context();
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::atomics::increment(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueAtomics),
            bucket,
            &key,
            delta,
        )
        .await?
        {
            Ok(n) => Ok(Ok(n)),
            Err(err) => Ok(Err(err.into())),
        }
    }
}

impl<H> store::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument]
    async fn open(&mut self, name: String) -> anyhow::Result<Result<Resource<store::Bucket>>> {
        self.attach_parent_context();
        let bucket = self
            .table
            .push(Arc::from(name))
            .context("failed to open bucket")?;
        Ok(Ok(bucket))
    }
}

impl<H> batch::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument(skip_all, fields(num_keys = keys.len()))]
    async fn get_many(
        &mut self,
        bucket: Resource<store::Bucket>,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<Vec<Option<(String, Vec<u8>)>>>> {
        self.attach_parent_context();
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        // NOTE(thomastaylor312): I don't like allocating a new vec, but I need borrowed strings to
        // have the right type
        let keys = keys.iter().map(String::as_str).collect::<Vec<_>>();

        match wrpc::wrpc::keyvalue::batch::get_many(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueBatch),
            bucket,
            &keys,
        )
        .await?
        {
            Ok(res) => Ok(Ok(res
                .into_iter()
                .map(|opt| opt.map(|(k, v)| (k, Vec::from(v))))
                .collect())),
            Err(err) => Err(err.into()),
        }
    }

    #[instrument(skip_all, fields(num_entries = entries.len()))]
    async fn set_many(
        &mut self,
        bucket: Resource<store::Bucket>,
        entries: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<Result<()>> {
        self.attach_parent_context();
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        let entries = entries
            .into_iter()
            .map(|(k, v)| (k, Bytes::from(v)))
            .collect::<Vec<_>>();
        let massaged = entries
            .iter()
            .map(|(k, v)| (k.as_str(), v))
            .collect::<Vec<_>>();
        match wrpc::wrpc::keyvalue::batch::set_many(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueBatch),
            bucket,
            &massaged,
        )
        .await?
        {
            Ok(()) => Ok(Ok(())),
            Err(err) => Err(err.into()),
        }
    }

    #[instrument(skip_all, fields(num_keys = keys.len()))]
    async fn delete_many(
        &mut self,
        bucket: Resource<store::Bucket>,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<()>> {
        self.attach_parent_context();
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        let keys = keys.iter().map(String::as_str).collect::<Vec<_>>();
        match wrpc::wrpc::keyvalue::batch::delete_many(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueBatch),
            bucket,
            &keys,
        )
        .await?
        {
            Ok(()) => Ok(Ok(())),
            Err(err) => Err(err.into()),
        }
    }
}

impl<H> store::HostBucket for Ctx<H>
where
    H: Handler,
{
    #[instrument]
    async fn get(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>>> {
        self.attach_parent_context();
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::get(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            &key,
        )
        .await?
        {
            Ok(buf) => Ok(Ok(buf.map(Into::into))),
            Err(err) => Ok(Err(err.into())),
        }
    }

    #[instrument]
    async fn set(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
        outgoing_value: Vec<u8>,
    ) -> anyhow::Result<Result<()>> {
        self.attach_parent_context();
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::set(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            &key,
            &Bytes::from(outgoing_value),
        )
        .await?
        {
            Ok(()) => Ok(Ok(())),
            Err(err) => Err(err.into()),
        }
    }

    #[instrument]
    async fn delete(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        self.attach_parent_context();
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::delete(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            &key,
        )
        .await?
        {
            Ok(()) => Ok(Ok(())),
            Err(err) => Err(err.into()),
        }
    }

    #[instrument]
    async fn exists(
        &mut self,
        bucket: Resource<store::Bucket>,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        self.attach_parent_context();
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::exists(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            &key,
        )
        .await?
        {
            Ok(ok) => Ok(Ok(ok)),
            Err(err) => Err(err.into()),
        }
    }

    #[instrument]
    async fn list_keys(
        &mut self,
        bucket: Resource<store::Bucket>,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<store::KeyResponse>> {
        self.attach_parent_context();
        let bucket = self.table.get(&bucket).context("failed to get bucket")?;
        match wrpc::wrpc::keyvalue::store::list_keys(
            &self.handler,
            Some(ReplacedInstanceTarget::KeyvalueStore),
            bucket,
            cursor,
        )
        .await?
        {
            Ok(wrpc::wrpc::keyvalue::store::KeyResponse { keys, cursor }) => {
                Ok(Ok(store::KeyResponse { keys, cursor }))
            }
            Err(err) => Err(err.into()),
        }
    }

    #[instrument]
    async fn drop(&mut self, bucket: Resource<store::Bucket>) -> anyhow::Result<()> {
        self.attach_parent_context();
        self.table
            .delete(bucket)
            .context("failed to delete bucket")?;
        Ok(())
    }
}

impl<H, C> wrpc::exports::wrpc::keyvalue::watcher::Handler<C> for Instance<H, C>
where
    H: Handler,
    C: Send,
{
    #[instrument(level = "info", skip_all)]
    async fn on_set(
        &self,
        _cx: C,
        bucket: String,
        key: String,
        value: bytes::Bytes,
    ) -> anyhow::Result<(), anyhow::Error> {
        let mut store = new_store(&self.engine, self.handler.clone(), self.max_execution_time);
        let pre = keyvalue_watcher_bindings::WatcherPre::new(self.pre.clone())
            .context("failed to pre-instantiate `wasi:keyvalue/watcher`")?;
        trace!("instantiating `wasi:keyvalue/watcher`");
        let bindings = pre
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate `wasi:keyvalue/watcher.on_set`")?;
        let bucket_repr: u32 = bucket.parse().context("failed to parse bucket as u32")?;
        let new_bucket = Resource::new_own(bucket_repr);
        debug!("invoking `wasi:keyvalue/watcher.on_set`");
        bindings
            .wasi_keyvalue_watcher()
            .call_on_set(&mut store, new_bucket, &key, &value)
            .await
            .context("failed to call `wasi:keyvalue/watcher.on_set`")?;
        Ok(())
    }

    #[instrument(level = "info", skip_all)]
    async fn on_delete(
        &self,
        _cx: C,
        bucket: String,
        key: String,
    ) -> anyhow::Result<(), anyhow::Error> {
        let mut store = new_store(&self.engine, self.handler.clone(), self.max_execution_time);
        let pre = keyvalue_watcher_bindings::WatcherPre::new(self.pre.clone())
            .context("failed to pre-instantiate `wasi:keyvalue/watcher`")?;
        trace!("instantiating `wasi:keyvalue/watcher`");
        let bindings = pre
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate `wasi:keyvalue/watcher.on_delete`")?;
        let bucket_repr: u32 = bucket.parse().context("failed to parse bucket as u32")?;
        let new_bucket = Resource::new_own(bucket_repr);
        debug!("invoking `wasi:keyvalue/watcher.on_delete`");
        bindings
            .wasi_keyvalue_watcher()
            .call_on_delete(&mut store, new_bucket, &key)
            .await
            .context("failed to call `wasi:keyvalue/watcher.on_delete`")?;
        Ok(())
    }
}

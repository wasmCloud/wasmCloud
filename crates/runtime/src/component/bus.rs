use core::fmt;

use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use tracing::instrument;
use wasmcloud_core::CallTargetInterface;
use wasmtime::component::Resource;
use wrpc_runtime_wasmtime::rpc;

use crate::capability::bus::{error, lattice};

use super::{Ctx, Handler, TableResult};

/// Wasmcloud error type
pub enum Error {
    /// Link was not found for target
    LinkNotFound(anyhow::Error),

    /// wRPC transport error
    Transport(rpc::Error),

    /// Handler error
    Handler(anyhow::Error),
}

impl std::error::Error for Error {}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::LinkNotFound(error) | Error::Handler(error) => error.fmt(f),
            Error::Transport(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::LinkNotFound(error) | Error::Handler(error) => error.fmt(f),
            Error::Transport(error) => error.fmt(f),
        }
    }
}

#[async_trait]
/// `wasmcloud:bus/lattice@2.0.1` implementation
pub trait Bus {
    /// Set the link name to use for a given list of interfaces, returning an error
    /// if a link doesn't exist on the given interfaces for the given target
    async fn set_link_name(
        &self,
        link_name: String,
        interfaces: Vec<Arc<CallTargetInterface>>,
    ) -> anyhow::Result<Result<(), String>>;
}

impl<H: Handler> lattice::Host for Ctx<H> {
    #[instrument(level = "debug", skip_all)]
    async fn set_link_name(
        &mut self,
        link_name: String,
        interfaces: Vec<Resource<Arc<CallTargetInterface>>>,
    ) -> anyhow::Result<Result<(), String>> {
        self.attach_parent_context();
        let interfaces = interfaces
            .into_iter()
            .map(|interface| self.table.get(&interface).cloned())
            .collect::<TableResult<_>>()
            .context("failed to convert call target interfaces")?;
        self.handler
            .set_link_name(link_name, interfaces)
            .await
            .context("failed to set link name")
    }
}

impl<H: Handler> lattice::HostCallTargetInterface for Ctx<H> {
    #[instrument(level = "debug", skip_all)]
    async fn new(
        &mut self,
        namespace: String,
        package: String,
        interface: String,
    ) -> anyhow::Result<Resource<Arc<CallTargetInterface>>> {
        self.attach_parent_context();
        self.table
            .push(Arc::new(CallTargetInterface {
                namespace,
                package,
                interface,
            }))
            .context("failed to push target interface")
    }

    async fn drop(&mut self, interface: Resource<Arc<CallTargetInterface>>) -> anyhow::Result<()> {
        self.table.delete(interface)?;
        Ok(())
    }
}

impl<H: Handler> error::Host for Ctx<H> {}

impl<H: Handler> error::HostError for Ctx<H> {
    async fn from_rpc_error(
        &mut self,
        error: Resource<rpc::Error>,
    ) -> wasmtime::Result<Resource<Error>> {
        let error = self
            .table
            .delete(error)
            .context("failed to delete `wrpc:rpc/error.error` from table")?;
        let error = match error {
            rpc::Error::Invoke(error) => match error.downcast() {
                Ok(error) => error,
                Err(error) => Error::Transport(rpc::Error::Invoke(error)),
            },
            rpc::Error::IncomingIndex(error) => match error.downcast() {
                Ok(error) => error,
                Err(error) => Error::Transport(rpc::Error::IncomingIndex(error)),
            },
            rpc::Error::OutgoingIndex(error) => match error.downcast() {
                Ok(error) => error,
                Err(error) => Error::Transport(rpc::Error::OutgoingIndex(error)),
            },
            rpc::Error::Stream(error) => Error::Transport(rpc::Error::Stream(error)),
        };
        let error = self
            .table
            .push(error)
            .context("failed to push error to table")?;
        Ok(error)
    }

    async fn from_io_error(
        &mut self,
        error: Resource<wasmtime_wasi::bindings::io::error::Error>,
    ) -> wasmtime::Result<
        Result<Resource<Error>, Resource<wasmtime_wasi::bindings::io::error::Error>>,
    > {
        let error = self
            .table
            .delete(error)
            .context("failed to delete `wasi:io/error.error` from table")?;
        match error.downcast() {
            Ok(error) => {
                let error = self
                    .table
                    .push(Error::Transport(rpc::Error::Stream(error)))
                    .context("failed to push error to table")?;
                Ok(Ok(error))
            }
            Err(error) => {
                let error = self
                    .table
                    .push(error)
                    .context("failed to push `wasi:io/error.error` to table")?;
                Ok(Err(error))
            }
        }
    }

    async fn to_debug_string(&mut self, error: Resource<Error>) -> wasmtime::Result<String> {
        let error = self
            .table
            .get(&error)
            .context("failed to get error from table")?;
        Ok(error.to_string())
    }

    async fn drop(&mut self, error: Resource<Error>) -> wasmtime::Result<()> {
        self.table
            .delete(error)
            .context("failed to delete error from table")?;
        Ok(())
    }
}

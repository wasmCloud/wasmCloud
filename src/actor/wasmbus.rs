use super::{guest_call, wasm};

use crate::capability;

use core::fmt::{self, Debug};
use std::ptr::NonNull;

use anyhow::{ensure, Context, Result};
use tracing::{instrument, trace, trace_span, warn};
use wascap::jwt;

pub struct Ctx<H> {
    console_log: Vec<String>,
    guest_call: Option<guest_call::State>,
    guest_error: Option<String>,
    guest_response: Option<Vec<u8>>,
    host_error: Option<String>,
    host_response: Option<Vec<u8>>,
    handler: H,
}

impl<H> Debug for Ctx<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("console_log", &self.console_log)
            .field("guest_call", &self.guest_call)
            .field("guest_error", &self.guest_error)
            .field("guest_response", &self.guest_response)
            .field("host_error", &self.host_error)
            .field("host_response", &self.host_response)
            .finish()
    }
}

impl<H> Ctx<H> {
    pub fn new(handler: H) -> Self {
        Self {
            console_log: Vec::default(),
            guest_call: None,
            guest_error: None,
            guest_response: None,
            host_error: None,
            host_response: None,
            handler,
        }
    }

    pub fn reset(&mut self) {
        self.console_log = Vec::default();
        self.guest_call = None;
        self.guest_error = None;
        self.guest_response = None;
        self.host_error = None;
        self.host_response = None;
    }

    pub fn set_guest_call(&mut self, operation: NonNull<[u8]>, payload: NonNull<[u8]>) {
        self.guest_call = Some((operation, payload));
    }

    pub fn take_console_log<T: FromIterator<String>>(&mut self) -> T {
        self.console_log.drain(..).collect()
    }

    pub fn take_guest_error(&mut self) -> Option<String> {
        self.guest_error.take()
    }

    pub fn take_guest_response(&mut self) -> Option<Vec<u8>> {
        self.guest_response.take()
    }

    pub fn take_host_error(&mut self) -> Option<String> {
        self.host_error.take()
    }
}

fn caller_memory<T>(store: &mut wasmtime::Caller<'_, T>) -> wasmtime::Memory {
    store
        .get_export("memory")
        .expect("`memory` not defined")
        .into_memory()
        .expect("`memory` type is not valid")
}

#[instrument(skip(store, memory))]
fn read_bytes(
    store: impl wasmtime::AsContext,
    memory: &wasmtime::Memory,
    data: wasm::ptr,
    len: wasm::usize,
) -> Result<Vec<u8>> {
    let data = data.try_into().context("pointer does not fit in usize")?;
    let len = len.try_into().context("size does not fit in usize")?;

    let mut buf = vec![0; len];
    memory
        .read(store, data, &mut buf)
        .context("failed to read data from guest memory")?;
    trace!(?buf, "read buffer from guest memory");
    Ok(buf)
}

#[instrument(skip(store, memory))]
fn read_string(
    store: impl wasmtime::AsContext,
    memory: &wasmtime::Memory,
    data: wasm::ptr,
    len: wasm::usize,
) -> Result<String> {
    let buf =
        read_bytes(store, memory, data, len).context("failed to read bytes from guest memory")?;
    let s = String::from_utf8(buf).context("failed to parse bytes as UTF-8")?;
    trace!(s, "read string from guest memory");
    Ok(s)
}

#[instrument(skip(store, memory, buf))]
fn write_bytes<T>(
    store: &mut wasmtime::Caller<'_, T>,
    memory: &wasmtime::Memory,
    data: wasm::ptr,
    buf: impl AsRef<[u8]>,
) -> Result<()> {
    let buf = buf.as_ref();
    let data = data.try_into().context("pointer does not fit in usize")?;
    memory
        .write(store, data, buf)
        .context("failed to write bytes to guest memory")?;
    trace!(?buf, "wrote bytes into guest memory");
    Ok(())
}

#[instrument(skip(store, err))]
fn set_host_error<H>(store: &mut wasmtime::Caller<'_, super::Ctx<'_, H>>, err: impl ToString) {
    let err = err.to_string();
    trace!(err, "set host error");
    store.data_mut().wasmbus.host_error = Some(err);
}

#[instrument(skip(store, res))]
fn set_host_response<H>(
    store: &mut wasmtime::Caller<'_, super::Ctx<'_, H>>,
    res: impl Into<Vec<u8>>,
) {
    let res = res.into();
    trace!(?res, "set host response");
    store.data_mut().wasmbus.host_response = Some(res);
}

#[instrument(skip(store))]
fn console_log<H>(
    mut store: wasmtime::Caller<'_, super::Ctx<'_, H>>,
    log_ptr: wasm::ptr,
    log_len: wasm::usize,
) -> Result<()> {
    let memory = caller_memory(&mut store);
    let log = read_string(&mut store, &memory, log_ptr, log_len)
        .context("failed to read `__console_log` log string")?;
    trace!(log, "store log string");
    store.data_mut().wasmbus.console_log.push(log);
    Ok(())
}

#[instrument(skip(store))]
fn guest_error<H>(
    mut store: wasmtime::Caller<'_, super::Ctx<'_, H>>,
    err_ptr: wasm::ptr,
    err_len: wasm::usize,
) -> Result<()> {
    let memory = caller_memory(&mut store);
    let err = read_string(&mut store, &memory, err_ptr, err_len)
        .context("failed to read `__guest_error` error string")?;
    trace!(err, "set guest error");
    store.data_mut().wasmbus.guest_error = Some(err);
    Ok(())
}

#[instrument(skip(store))]
fn guest_request<H>(
    mut store: wasmtime::Caller<'_, super::Ctx<'_, H>>,
    op_ptr: wasm::ptr,
    pld_ptr: wasm::ptr,
) -> Result<()> {
    let (op, pld) = store
        .data_mut()
        .wasmbus
        .guest_call
        .take()
        .context("unexpected `__guest_request`")?;

    let memory = caller_memory(&mut store);
    write_bytes(&mut store, &memory, op_ptr, unsafe { op.as_ref() })
        .context("failed to write `__guest_call` operation into guest memory")?;
    write_bytes(&mut store, &memory, pld_ptr, unsafe { pld.as_ref() })
        .context("failed to write `__guest_call` payload into guest memory")
}

#[instrument(skip(store))]
fn guest_response<H>(
    mut store: wasmtime::Caller<'_, super::Ctx<'_, H>>,
    res_ptr: wasm::ptr,
    res_len: wasm::usize,
) -> Result<()> {
    let memory = caller_memory(&mut store);
    let res = read_bytes(&mut store, &memory, res_ptr, res_len)
        .context("failed to read `__guest_response` response")?;
    trace!(?res, "set guest response");
    store.data_mut().wasmbus.guest_response = Some(res);
    Ok(())
}

#[instrument(skip(store))]
#[allow(clippy::too_many_arguments)]
fn host_call<H: capability::Handler>(
    mut store: wasmtime::Caller<'_, super::Ctx<'_, H>>,
    bd_ptr: wasm::ptr,
    bd_len: wasm::usize,
    ns_ptr: wasm::ptr,
    ns_len: wasm::usize,
    op_ptr: wasm::ptr,
    op_len: wasm::usize,
    pld_ptr: wasm::ptr,
    pld_len: wasm::usize,
) -> Result<wasm::usize> {
    let memory = caller_memory(&mut store);
    let bd = read_string(&mut store, &memory, bd_ptr, bd_len)
        .context("failed to read `__host_call` binding")?;

    let ns = read_string(&mut store, &memory, ns_ptr, ns_len)
        .context("failed to read `__host_call` namespace")?;

    let op = read_string(&mut store, &memory, op_ptr, op_len)
        .context("failed to read `__host_call` operation")?;

    let pld = read_bytes(&mut store, &memory, pld_ptr, pld_len)
        .context("failed to read `__host_call` payload")?;

    ensure!(
        store
            .data()
            .claims
            .metadata
            .as_ref()
            .map(|jwt::Actor { caps, .. }| caps.as_ref().map(|caps| caps.contains(&ns)))
            .unwrap_or_default()
            .unwrap_or_default(),
        "`{ns}` capability request unauthorized"
    );

    match trace_span!("capability::Handler::handle", bd, ns, op, ?pld)
        .in_scope(|| {
            let ctx = store.data();
            ctx.wasmbus.handler.handle(ctx.claims, bd, ns, op, pld)
        })
        .context("failed to handle provider invocation")?
    {
        Ok(buf) => {
            set_host_response(&mut store, buf);
            Ok(wasm::SUCCESS)
        }
        Err(err) => {
            set_host_error(&mut store, err);
            Ok(wasm::ERROR)
        }
    }
}

#[instrument(skip(store))]
fn host_error<H>(
    mut store: wasmtime::Caller<'_, super::Ctx<'_, H>>,
    err_ptr: wasm::ptr,
) -> Result<()> {
    let err = store
        .data_mut()
        .wasmbus
        .host_error
        .take()
        .context("unexpected `__host_error`")?;

    let memory = caller_memory(&mut store);
    trace_span!("write error into guest memory", err)
        .in_scope(|| write_bytes(&mut store, &memory, err_ptr, err.as_bytes()))
        .context("failed to write `__host_error` error into guest memory")
}

#[instrument(skip(store))]
fn host_error_len<H>(store: wasmtime::Caller<'_, super::Ctx<'_, H>>) -> wasm::usize {
    let len = store
        .data()
        .wasmbus
        .host_error
        .as_ref()
        .map(String::as_bytes)
        .map(<[_]>::len)
        .unwrap_or_default()
        .try_into()
        .unwrap_or_else(|_| {
            warn!(
                "`host_error_len` does not fit in i32, truncating to {}",
                wasm::usize::MAX
            );
            wasm::usize::MAX
        });
    trace!(len);
    len
}

#[instrument(skip(store))]
fn host_response<H>(
    mut store: wasmtime::Caller<'_, super::Ctx<'_, H>>,
    res_ptr: wasm::ptr,
) -> Result<()> {
    let res = store
        .data_mut()
        .wasmbus
        .host_response
        .take()
        .context("unexpected `__host_response`")?;

    let memory = caller_memory(&mut store);
    trace_span!("write response into guest memory", ?res)
        .in_scope(|| write_bytes(&mut store, &memory, res_ptr, res))
        .context("failed to write `__host_response` response into guest memory")
}

#[instrument(skip(store))]
fn host_response_len<H>(store: wasmtime::Caller<'_, super::Ctx<'_, H>>) -> wasm::usize {
    let len = store
        .data()
        .wasmbus
        .host_response
        .as_ref()
        .map(Vec::len)
        .unwrap_or_default()
        .try_into()
        .unwrap_or_else(|_| {
            warn!(
                "`host_response_len` does not fit in i32, truncating to {}",
                wasm::usize::MAX
            );
            wasm::usize::MAX
        });
    trace!(len);
    len
}

pub(super) fn add_to_linker(
    linker: &mut wasmtime::Linker<super::Ctx<'_, impl capability::Handler + 'static>>,
) -> Result<()> {
    linker.func_wrap("wasmbus", "__console_log", console_log)?;
    linker.func_wrap("wasmbus", "__guest_error", guest_error)?;
    linker.func_wrap("wasmbus", "__guest_request", guest_request)?;
    linker.func_wrap("wasmbus", "__guest_response", guest_response)?;
    linker.func_wrap("wasmbus", "__host_call", host_call)?;
    linker.func_wrap("wasmbus", "__host_error", host_error)?;
    linker.func_wrap("wasmbus", "__host_error_len", host_error_len)?;
    linker.func_wrap("wasmbus", "__host_response", host_response)?;
    linker.func_wrap("wasmbus", "__host_response_len", host_response_len)?;
    Ok(())
}

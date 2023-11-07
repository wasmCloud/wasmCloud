use crate::capability::builtin::{self, Bus, Logging};
use crate::capability::logging::logging;

use core::fmt::{self, Debug};

use anyhow::{bail, Context, Result};
use rand::{thread_rng, Rng, RngCore};
use tracing::{instrument, trace, trace_span, warn};

pub mod guest_call {
    use super::wasm;

    pub type Params = (wasm::usize, wasm::usize);
    pub type Result = wasm::usize;
}

mod wasm {
    #[allow(non_camel_case_types)]
    pub type ptr = u32;
    #[allow(non_camel_case_types)]
    pub type usize = u32;

    pub const ERROR: usize = usize::MAX;
    pub const SUCCESS: usize = 1;
}

pub struct Ctx {
    console_log: Vec<String>,
    guest_call: Option<(String, Vec<u8>)>,
    guest_error: Option<String>,
    guest_response: Option<Vec<u8>>,
    host_error: Option<String>,
    host_response: Option<Vec<u8>>,
    pub(crate) handler: builtin::Handler,
}

impl Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("console_log", &self.console_log)
            .field("guest_call", &self.guest_call)
            .field("guest_error", &self.guest_error)
            .field("guest_response", &self.guest_response)
            .field("host_error", &self.host_error)
            .field("host_response", &self.host_response)
            .finish_non_exhaustive()
    }
}

impl Ctx {
    pub(super) fn new(handler: impl Into<builtin::Handler>) -> Self {
        Self {
            console_log: Vec::default(),
            guest_call: None,
            guest_error: None,
            guest_response: None,
            host_error: None,
            host_response: None,
            handler: handler.into(),
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

    pub fn set_guest_call(&mut self, operation: String, payload: Vec<u8>) {
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

#[instrument(level = "trace", skip(store, memory))]
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
    trace!("read buffer from guest memory");
    Ok(buf)
}

#[instrument(level = "trace", skip(store, memory))]
fn read_string(
    store: impl wasmtime::AsContext,
    memory: &wasmtime::Memory,
    data: wasm::ptr,
    len: wasm::usize,
) -> Result<String> {
    let buf =
        read_bytes(store, memory, data, len).context("failed to read bytes from guest memory")?;
    let s = String::from_utf8(buf).context("failed to parse bytes as UTF-8")?;
    trace!("read string from guest memory");
    Ok(s)
}

#[instrument(level = "trace", skip(store, memory, buf))]
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
    trace!(len = buf.len(), "wrote bytes into guest memory");
    Ok(())
}

#[instrument(level = "trace", skip(store, err))]
fn set_host_error(store: &mut wasmtime::Caller<'_, super::Ctx>, err: String) {
    trace!(err, "set host error");
    store.data_mut().wasmbus.host_error = Some(err);
}

#[instrument(level = "trace", skip(store, res))]
fn set_host_response(store: &mut wasmtime::Caller<'_, super::Ctx>, res: impl Into<Vec<u8>>) {
    let res = res.into();
    trace!("set host response");
    store.data_mut().wasmbus.host_response = Some(res);
}

#[instrument(level = "trace", skip(store))]
fn console_log(
    mut store: wasmtime::Caller<'_, super::Ctx>,
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

#[instrument(level = "trace", skip(store))]
fn guest_error(
    mut store: wasmtime::Caller<'_, super::Ctx>,
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

#[instrument(level = "trace", skip(store))]
fn guest_request(
    mut store: wasmtime::Caller<'_, super::Ctx>,
    operation_ptr: wasm::ptr,
    payload_ptr: wasm::ptr,
) -> Result<()> {
    let (op, pld) = store
        .data_mut()
        .wasmbus
        .guest_call
        .take()
        .context("unexpected `__guest_request`")?;

    let memory = caller_memory(&mut store);
    write_bytes(&mut store, &memory, operation_ptr, op)
        .context("failed to write `__guest_call` operation into guest memory")?;
    write_bytes(&mut store, &memory, payload_ptr, pld)
        .context("failed to write `__guest_call` payload into guest memory")
}

#[instrument(level = "trace", skip(store))]
fn guest_response(
    mut store: wasmtime::Caller<'_, super::Ctx>,
    res_ptr: wasm::ptr,
    res_len: wasm::usize,
) -> Result<()> {
    let memory = caller_memory(&mut store);
    let res = read_bytes(&mut store, &memory, res_ptr, res_len)
        .context("failed to read `__guest_response` response")?;
    trace!("set guest response");
    store.data_mut().wasmbus.guest_response = Some(res);
    Ok(())
}

#[instrument(level = "trace", skip(handler, payload))]
async fn handle(
    handler: &mut builtin::Handler,
    binding: String,
    namespace: String,
    operation: String,
    payload: Vec<u8>,
) -> anyhow::Result<Vec<u8>> {
    match (namespace.as_str(), operation.as_str()) {
        ("wasmcloud:builtin:logging", "Logging.WriteLog") => {
            let wasmcloud_compat::logging::LogEntry { level, text } =
                rmp_serde::from_slice(&payload).context("failed to deserialize log entry")?;
            let level = match level.as_str() {
                "trace" => logging::Level::Trace,
                "debug" => logging::Level::Debug,
                "info" => logging::Level::Info,
                "warn" => logging::Level::Warn,
                "error" => logging::Level::Error,
                "critical" => logging::Level::Critical,
                level => bail!("unsupported log level `{level}`"),
            };
            handler.log(level, String::new(), text).await?;
            Ok(vec![])
        }
        ("wasmcloud:builtin:numbergen", "NumberGen.GenerateGuid") => {
            let mut buf = uuid::Bytes::default();
            thread_rng()
                .try_fill_bytes(&mut buf)
                .context("failed to fill buffer")?;
            let guid = uuid::Builder::from_random_bytes(buf).into_uuid();
            rmp_serde::to_vec(&guid.to_string()).context("failed to serialize GUID string")
        }
        ("wasmcloud:builtin:numbergen", "NumberGen.Random32") => {
            let v = thread_rng().next_u32();
            rmp_serde::to_vec(&v).context("failed to serialize u32")
        }
        ("wasmcloud:builtin:numbergen", "NumberGen.RandomInRange") => {
            let wasmcloud_compat::numbergen::RangeLimit { min, max } =
                rmp_serde::from_slice(&payload).context("failed to deserialize range limit")?;
            let v = thread_rng().gen_range(min..=max);
            rmp_serde::to_vec(&v).context("failed to serialize u32")
        }
        _ => {
            let target = handler
                .identify_wasmbus_target(&binding, &namespace)
                .await
                .context("failed to identify invocation target")?;
            handler
                .call_sync(Some(target), format!("{namespace}/{operation}"), payload)
                .await
                .context("failed to call `wasmcloud:bus/host.call`")
        }
    }
}

#[instrument(level = "trace", skip(store))]
#[allow(clippy::too_many_arguments)]
async fn host_call(
    mut store: wasmtime::Caller<'_, super::Ctx>,
    binding_ptr: wasm::ptr,
    binding_len: wasm::usize,
    namespace_ptr: wasm::ptr,
    namespace_len: wasm::usize,
    operation_ptr: wasm::ptr,
    operation_len: wasm::usize,
    payload_ptr: wasm::ptr,
    payload_len: wasm::usize,
) -> Result<wasm::usize> {
    let memory = caller_memory(&mut store);
    let bd = read_string(&mut store, &memory, binding_ptr, binding_len)
        .context("failed to read `__host_call` binding")?;

    let ns = read_string(&mut store, &memory, namespace_ptr, namespace_len)
        .context("failed to read `__host_call` namespace")?;

    let op = read_string(&mut store, &memory, operation_ptr, operation_len)
        .context("failed to read `__host_call` operation")?;

    let pld = read_bytes(&mut store, &memory, payload_ptr, payload_len)
        .context("failed to read `__host_call` payload")?;
    match handle(&mut store.data_mut().wasmbus.handler, bd, ns, op, pld).await {
        Ok(res) => {
            set_host_response(&mut store, res);
            Ok(wasm::SUCCESS)
        }
        Err(err) => {
            set_host_error(&mut store, format!("{err:#}"));
            Ok(wasm::ERROR)
        }
    }
}

#[instrument(level = "trace", skip(store))]
fn host_error(mut store: wasmtime::Caller<'_, super::Ctx>, err_ptr: wasm::ptr) -> Result<()> {
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

#[instrument(level = "trace", skip(store))]
fn host_error_len(store: wasmtime::Caller<'_, super::Ctx>) -> wasm::usize {
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
                "`host_error_len` does not fit in u32, truncating to {}",
                wasm::usize::MAX
            );
            wasm::usize::MAX
        });
    trace!(len, "`host_error_len` called");
    len
}

#[instrument(level = "trace", skip(store))]
fn host_response(mut store: wasmtime::Caller<'_, super::Ctx>, res_ptr: wasm::ptr) -> Result<()> {
    let res = store
        .data_mut()
        .wasmbus
        .host_response
        .take()
        .context("unexpected `__host_response`")?;

    let memory = caller_memory(&mut store);
    trace_span!("write response into guest memory")
        .in_scope(|| write_bytes(&mut store, &memory, res_ptr, res))
        .context("failed to write `__host_response` response into guest memory")
}

#[instrument(level = "trace", skip(store))]
fn host_response_len(store: wasmtime::Caller<'_, super::Ctx>) -> wasm::usize {
    store
        .data()
        .wasmbus
        .host_response
        .as_ref()
        .map(Vec::len)
        .unwrap_or_default()
        .try_into()
        .unwrap_or_else(|_| {
            warn!(
                "`host_response_len` does not fit in u32, truncating to {}",
                wasm::usize::MAX
            );
            wasm::usize::MAX
        })
}

pub(super) fn add_to_linker(linker: &mut wasmtime::Linker<super::Ctx>) -> Result<()> {
    linker.func_wrap("wasmbus", "__console_log", console_log)?;
    linker.func_wrap("wasmbus", "__guest_error", guest_error)?;
    linker.func_wrap("wasmbus", "__guest_request", guest_request)?;
    linker.func_wrap("wasmbus", "__guest_response", guest_response)?;
    linker.func_wrap8_async(
        "wasmbus",
        "__host_call",
        |store,
         binding_ptr,
         binding_len,
         namespace_ptr,
         namespace_len,
         operation_ptr,
         operation_len,
         payload_ptr,
         payload_len| {
            Box::new(host_call(
                store,
                binding_ptr,
                binding_len,
                namespace_ptr,
                namespace_len,
                operation_ptr,
                operation_len,
                payload_ptr,
                payload_len,
            ))
        },
    )?;
    linker.func_wrap("wasmbus", "__host_error", host_error)?;
    linker.func_wrap("wasmbus", "__host_error_len", host_error_len)?;
    linker.func_wrap("wasmbus", "__host_response", host_response)?;
    linker.func_wrap("wasmbus", "__host_response_len", host_response_len)?;
    Ok(())
}

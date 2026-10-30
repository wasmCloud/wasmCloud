//! One-shot execution of a workload component's `wasi:cli/run` export.

use anyhow::Context as _;
use wasmtime::component::types::ComponentItem;
use wasmtime_wasi::{
    WasiCtxBuilder,
    cli::{StdinStream, StdoutStream},
};

use super::{linked_call::apply_environment, workload::ResolvedWorkload};

/// Invocation-local command arguments and standard streams.
///
/// Arguments include the program name (`argv[0]`). By default stdin is closed
/// and stdout/stderr are inherited. Streams are consumed by one invocation;
/// they are never shared with linked components or replayed after a failure.
///
/// Build one with [`CommandOptions::new`] and the chained setters, or start
/// from [`Default::default`] and assign fields. The struct is
/// `#[non_exhaustive]` so that later options do not break callers.
#[derive(Default)]
#[non_exhaustive]
pub struct CommandOptions {
    pub args: Vec<String>,
    pub stdin: Option<Box<dyn StdinStream + Sync>>,
    pub stdout: Option<Box<dyn StdoutStream + Sync>>,
    pub stderr: Option<Box<dyn StdoutStream + Sync>>,
}

impl CommandOptions {
    /// No arguments, stdin at EOF, stdout and stderr inherited.
    pub fn new() -> Self {
        Self::default()
    }

    /// The full argument vector, program name first.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Serve the command's stdin from `stdin` instead of EOF.
    pub fn stdin(mut self, stdin: impl StdinStream + Sync + 'static) -> Self {
        self.stdin = Some(Box::new(stdin));
        self
    }

    /// Send the command's stdout to `stdout` instead of the host's.
    pub fn stdout(mut self, stdout: impl StdoutStream + Sync + 'static) -> Self {
        self.stdout = Some(Box::new(stdout));
        self
    }

    /// Send the command's stderr to `stderr` instead of the host's.
    pub fn stderr(mut self, stderr: impl StdoutStream + Sync + 'static) -> Self {
        self.stderr = Some(Box::new(stderr));
        self
    }
}

/// A normal command exit. A trap is returned as an error instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommandOutcome {
    pub exit_code: i32,
}

impl ResolvedWorkload {
    /// Run one component to completion in a fresh store, without restarting it.
    ///
    /// Uses the workload's resolved grants, plugins, environment and volumes.
    /// Supports both WASIp2 and WASIp3 commands; a component exporting both
    /// runs as WASIp3. Dropping this future drops the command's store. This
    /// does not start the workload's service or ingresses; embedders resolve a
    /// workload, run commands, then unbind its plugins.
    pub async fn run_command(
        &self,
        component_id: &str,
        options: CommandOptions,
    ) -> anyhow::Result<CommandOutcome> {
        anyhow::ensure!(!self.released(), "workload is stopping");
        let pre = self.instantiate_pre(component_id).await?;
        // Select by the run export, not by imports: a p2 command may import a
        // p3 capability without changing its command entry point.
        let component = pre.component();
        let exports = component.component_type();
        let p3 = exports.exports(component.engine()).any(|(name, item)| {
            name.starts_with("wasi:cli/run@0.3.")
                && matches!(item.ty, ComponentItem::ComponentInstance(_))
        });
        let p2 = exports.exports(component.engine()).any(|(name, item)| {
            name.starts_with("wasi:cli/run@0.2.")
                && matches!(item.ty, ComponentItem::ComponentInstance(_))
        });
        anyhow::ensure!(p2 || p3, "component exports no supported wasi:cli/run");

        // The store built below already carries the component's environment
        // (see `build_ctx_from_template`), but `WasiCliCtx` is only settable
        // through its builder, so giving this invocation its own arguments and
        // streams means rebuilding the whole CLI context. `apply_environment`
        // keeps both paths on the same variables.
        let mut cli = WasiCtxBuilder::new();
        {
            let components = self.components().read_owned().await;
            let component = components
                .get(component_id)
                .context("component ID not found in workload")?;
            apply_environment(
                &mut cli,
                &component.metadata().local_resources().environment,
            );
        }
        cli.args(&options.args).inherit_stdout().inherit_stderr();
        if let Some(stdin) = options.stdin {
            cli.stdin(stdin);
        }
        if let Some(stdout) = options.stdout {
            cli.stdout(stdout);
        }
        if let Some(stderr) = options.stderr {
            cli.stderr(stderr);
        }

        let mut store = self.new_store(component_id).await?;
        // Only replace CLI state; keep the resolved filesystem and network
        // contexts and their retained directory handles intact.
        *store.data_mut().active_ctx.ctx.cli() = std::mem::take(cli.build().cli());
        let result = if p3 {
            let command = wasmtime_wasi::p3::bindings::CommandPre::new(pre)?
                .instantiate_async(&mut store)
                .await
                .map_err(|e| {
                    crate::engine::pool_usage::explain_instantiate_failure(store.data(), e)
                })?;
            store
                .run_concurrent(async move |accessor| {
                    command.wasi_cli_run().call_run(accessor).await
                })
                .await
                .and_then(|result| result)
        } else {
            let command = wasmtime_wasi::p2::bindings::CommandPre::new(pre)?
                .instantiate_async(&mut store)
                .await
                .map_err(|e| {
                    crate::engine::pool_usage::explain_instantiate_failure(store.data(), e)
                })?;
            command.wasi_cli_run().call_run(&mut store).await
        };
        match result {
            Ok(Ok(())) => Ok(CommandOutcome { exit_code: 0 }),
            Ok(Err(())) => Ok(CommandOutcome { exit_code: 1 }),
            Err(error) => match error.downcast_ref::<wasmtime_wasi::I32Exit>() {
                Some(exit) => Ok(CommandOutcome { exit_code: exit.0 }),
                None => Err(anyhow::Error::from(error).context("command trapped")),
            },
        }
    }
}

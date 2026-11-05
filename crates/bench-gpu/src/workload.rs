//! One `wasi:webgpu` command run as a wasmCloud workload, start to finish.

use std::{
    collections::HashMap,
    num::NonZeroUsize,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use serde::Serialize;
use wash_runtime::{
    engine::{Engine, command::CommandOptions, guest_memory::GuestMemoryMode},
    host::HostRef,
    plugin::{
        HostPlugin,
        wasi_webgpu::{WebGpu, WebGpuBackend},
    },
    types::{Component, HostPathVolume, LocalResources, Volume, VolumeMount, VolumeType, Workload},
    wit::WitInterface,
};

/// The `wasi:webgpu` the host links, as the workload must ask for it.
const WEBGPU_INTERFACE: &str = "wasi:webgpu/webgpu@0.3.0-rc.2";

/// A host directory the guest sees at `guest`, read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub host: PathBuf,
    pub guest: String,
}

impl std::str::FromStr for Mount {
    type Err = anyhow::Error;

    fn from_str(spec: &str) -> Result<Self> {
        let Some((host, guest)) = spec.rsplit_once(':') else {
            bail!("expected HOST:GUEST, got {spec:?}");
        };
        if host.is_empty() || !guest.starts_with('/') {
            bail!("expected HOST:GUEST with an absolute GUEST path, got {spec:?}");
        }
        Ok(Self {
            host: host.into(),
            guest: guest.into(),
        })
    }
}

/// How the host the commands run in is set up.
#[derive(Debug, clap::Args)]
pub struct Options {
    /// A host directory to mount read-only, as HOST:GUEST. Repeatable.
    #[arg(long = "dir", value_name = "HOST:GUEST")]
    pub mounts: Vec<Mount>,
    /// Bytes a guest may pass the host in one call. Uploading model weights
    /// moves whole tensors per call, past wasmtime's 128 MiB default.
    #[arg(long, default_value_t = NonZeroUsize::new(8 << 30).unwrap_or(NonZeroUsize::MIN))]
    pub hostcall_fuel: NonZeroUsize,
    /// Use wgpu's no-op backend: checks wiring without a GPU.
    #[arg(long)]
    pub noop: bool,
}

/// An engine and `wasi:webgpu` plugin shared by every run, so the GPU
/// instance outlives each workload the way it does in a host.
pub struct Runner {
    engine: Engine,
    plugins: HashMap<&'static str, Arc<dyn HostPlugin>>,
    mounts: Vec<Mount>,
}

/// What one run measured.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    /// `None` when the guest trapped instead of exiting.
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    /// Compile, link and plugin bind: a host's workload start.
    pub resolve_ms: f64,
    /// `wasi:cli/run`, from call to return.
    pub run_ms: f64,
}

impl Runner {
    pub fn new(options: Options) -> Result<Self> {
        let mounts = options
            .mounts
            .into_iter()
            .map(|m| {
                let host = m
                    .host
                    .canonicalize()
                    .with_context(|| format!("mount {}", m.host.display()))?;
                Ok(Mount { host, ..m })
            })
            .collect::<Result<Vec<_>>>()?;
        // Guest memory is counted rather than enforced, and the process
        // footprint is not policed: the bench reports what a model needs,
        // and on unified-memory GPUs device allocations land in the process.
        let engine = Engine::builder()
            .with_pooling_allocator(false)
            .with_hostcall_fuel(options.hostcall_fuel)
            .with_guest_memory_mode(GuestMemoryMode::Count)
            .without_max_process_memory()
            .with_allowed_host_paths(mounts.iter().map(|m| m.host.clone()))
            .build()?;
        let backend = if options.noop {
            WebGpuBackend::Noop
        } else {
            WebGpuBackend::All
        };
        let webgpu = WebGpu::new(backend);
        let webgpu: Arc<dyn HostPlugin> = Arc::new(webgpu);
        Ok(Self {
            engine,
            plugins: HashMap::from([(webgpu.id(), webgpu)]),
            mounts,
        })
    }

    /// The most guest memory charged at once since the engine was built.
    pub fn guest_memory_peak(&self) -> u64 {
        self.engine.guest_memory().high_water()
    }

    /// Start `wasm` as a fresh workload, run it once with `args` (program
    /// name first), and stop it, dropping its store and GPU objects.
    pub async fn run(&self, name: &str, wasm: Vec<u8>, args: &[String]) -> Result<Outcome> {
        let started = Instant::now();
        let volumes = self
            .mounts
            .iter()
            .enumerate()
            .map(|(i, m)| Volume {
                name: format!("mount-{i}"),
                volume_type: VolumeType::HostPath(HostPathVolume {
                    local_path: m.host.to_string_lossy().into_owned(),
                }),
            })
            .collect();
        let volume_mounts = self
            .mounts
            .iter()
            .enumerate()
            .map(|(i, m)| VolumeMount {
                name: format!("mount-{i}"),
                mount_path: m.guest.clone(),
                read_only: true,
            })
            .collect();
        let workload = Workload {
            namespace: "gpubench".into(),
            name: name.into(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: name.into(),
                bytes: wasm.into(),
                local_resources: LocalResources {
                    volume_mounts,
                    ..Default::default()
                },
                ..Default::default()
            }],
            host_interfaces: vec![WitInterface::from(WEBGPU_INTERFACE)],
            volumes,
        };
        let resolved = self
            .engine
            .initialize_workload(name, workload)?
            .resolve(
                Some(&self.plugins),
                &Default::default(),
                &HostRef::detached(),
                &Default::default(),
            )
            .await?;
        let component_id = resolved
            .components()
            .read_owned()
            .await
            .keys()
            .next()
            .cloned()
            .context("workload has no component")?;
        let resolve_ms = ms(started.elapsed());

        let running = Instant::now();
        let result = resolved
            .run_command(
                &component_id,
                CommandOptions::new().args(args.iter().cloned()),
            )
            .await;
        let run_ms = ms(running.elapsed());
        let unbound = resolved.unbind_all_plugins().await;
        drop(resolved);
        if let Err(err) = &unbound {
            tracing::warn!(workload = name, error = %format!("{err:#}"), "unbinding plugins failed");
        }

        Ok(match result {
            Ok(outcome) => Outcome {
                exit_code: Some(outcome.exit_code),
                error: None,
                resolve_ms,
                run_ms,
            },
            Err(err) => Outcome {
                exit_code: None,
                error: Some(format!("{err:#}")),
                resolve_ms,
                run_ms,
            },
        })
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

#[cfg(test)]
mod tests {
    use super::Mount;

    #[test]
    fn mount_splits_on_the_last_colon() -> anyhow::Result<()> {
        let m: Mount = "/a:b/models:/models".parse()?;
        assert_eq!(m.host.to_str(), Some("/a:b/models"));
        assert_eq!(m.guest, "/models");
        Ok(())
    }

    #[test]
    fn mount_rejects_relative_guest_paths() {
        assert!("/models:models".parse::<Mount>().is_err());
        assert!(":/models".parse::<Mount>().is_err());
        assert!("/models".parse::<Mount>().is_err());
    }
}

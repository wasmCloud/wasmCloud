//! How many HTTP workloads one wasmCloud host holds with the CPU and memory
//! the machine gives it, warm and scaled to zero. See README.md.
//!
//! The bench talks to the host only over its NATS API and HTTP, so any host
//! binary that speaks that API can be measured and compared.

mod api;
mod bench;
mod cli;
mod host;
mod memory;
mod provision;
mod registry;
mod report;
mod size;
mod stats;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use clap::Parser as _;

use crate::bench::{Flavour, HostInfo, Run, Settings, Target};
use crate::cli::{Args, FlavourArg, HostCli};
use crate::host::{HostLimits, Pick};
use crate::memory::Sampler;
use crate::registry::{Image, Registry, RegistryCounts};
use crate::report::{ImageInfo, Machine, Report};

/// Exit status when a flavour served fewer than --min-components.
const EXIT_TOO_FEW: u8 = 2;

fn main() -> ExitCode {
    let args = Args::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("bench-scale: starting the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Ctrl-C drops the run, and with it every child-process guard, so no
    // host or nats-server is left behind.
    let result = runtime.block_on(async {
        tokio::select! {
            result = run(args) => result,
            _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("interrupted")),
        }
    });
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("bench-scale: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn resolve_settings(args: &Args, machine: &Machine) -> Result<Settings> {
    if args.step == 0 || args.start_concurrency == 0 || args.request_concurrency == 0 {
        bail!("--step, --start-concurrency and --request-concurrency must be at least 1");
    }
    if args.overshoot.is_nan() || args.overshoot < 1.0 {
        bail!("--overshoot must be at least 1.0");
    }
    let memory_limit_bytes = match &args.memory_limit {
        Some(limit) => size::parse(limit).context("--memory-limit")?,
        None if machine.memory_total_bytes > 0 => machine.memory_total_bytes / 2,
        None => bail!("cannot read physical memory; pass --memory-limit"),
    };
    let core_instances = args.core_instances.unwrap_or_else(|| {
        provision::derive_core_instances(
            provision::usable_address_space(),
            provision::max_map_count(),
        )
    });
    Ok(Settings {
        memory_limit_bytes,
        max_components: args.max_components,
        overshoot: args.overshoot,
        step: args.step,
        start_concurrency: args.start_concurrency,
        start_timeout_secs: args.start_timeout,
        start_retries: args.start_retries,
        sample: args.sample,
        request_concurrency: args.request_concurrency,
        request_timeout_secs: args.request_timeout,
        spin_down_wait_ms: args.spin_down_wait,
        core_instances,
        core_instances_per_workload: args.core_instances_per_workload,
        min_components: args.min_components,
    })
}

/// Everything a flavour needs to bring up (or find) its host.
struct HostPlan<'a> {
    args: &'a Args,
    nats: &'a async_nats::Client,
    nats_url: &'a str,
    binary: Option<&'a Path>,
    flags: &'a std::collections::HashSet<String>,
    limits: HostLimits,
    run_dir: &'a Path,
    run_id: &'a str,
}

async fn prepare_target(plan: &HostPlan<'_>, flavour: Flavour) -> Result<Target> {
    let args = plan.args;
    let timeout = Duration::from_secs(args.heartbeat_timeout);
    let mut heartbeats = host::Heartbeats::subscribe(plan.nats).await?;
    let (heartbeat, guard, log, info_args, unsupported, pid, core_instances) = match plan.binary {
        None => {
            let pick = args.host_id.as_deref().map_or(Pick::Any, Pick::Id);
            let heartbeat = heartbeats.wait(pick, timeout, None).await.context(
                "waiting for an attached host; it heartbeats every 15s, so allow at least that",
            )?;
            (
                heartbeat,
                None,
                None,
                Vec::new(),
                Vec::new(),
                args.host_pid,
                None,
            )
        }
        Some(binary) => {
            let host_name = format!("bench-scale-{}-{}", plan.run_id, flavour.name());
            let command = host::host_command(
                args.host_cli,
                plan.flags,
                plan.nats_url,
                &plan.limits,
                &plan.run_dir.join(format!("oci-cache-{}", flavour.name())),
                &host_name,
                &args.host_args,
            );
            let log = plan.run_dir.join(format!("host-{}.log", flavour.name()));
            let mut guard = host::spawn_host(binary, &command, &args.host_log, &log)?;
            let pick = match (&args.host_id, &command.host_name, args.nats_url.is_some()) {
                (Some(id), _, _) => Pick::Id(id),
                (None, Some(name), _) => Pick::Name(name),
                // A NATS of our own has no other host on it.
                (None, None, false) => Pick::Any,
                (None, None, true) => bail!(
                    "on a shared NATS, a host without --host-name needs --host-id to be told apart"
                ),
            };
            let heartbeat = heartbeats
                .wait(pick, timeout, Some((&mut guard, &log)))
                .await?;
            let pid = guard.pid();
            let core_instances = (args.host_cli == HostCli::Wash
                && !command
                    .unsupported_flags
                    .iter()
                    .any(|f| f == "core-instances"))
            .then_some(plan.limits.core_instances);
            (
                heartbeat,
                Some(Arc::new(Mutex::new(guard))),
                Some(log),
                command.args,
                command.unsupported_flags,
                Some(pid),
                core_instances,
            )
        }
    };
    let http = match &args.host_http {
        Some(addr) => addr.clone(),
        None if heartbeat.http_port > 0 => format!("127.0.0.1:{}", heartbeat.http_port),
        None => bail!(
            "host {} reports no httpPort; pass --host-http",
            heartbeat.id
        ),
    };
    let client = api::HostClient::new(plan.nats.clone(), heartbeat.id.clone());
    client.wait_ready(Duration::from_secs(30)).await?;
    Ok(Target {
        client,
        info: HostInfo {
            binary: plan.binary.map(|b| b.display().to_string()),
            args: info_args,
            unsupported_flags: unsupported,
            id: heartbeat.id.clone(),
            version: heartbeat.version.clone(),
            hostname: heartbeat.hostname.clone(),
            http,
            pid,
            log: log.as_ref().map(|l| l.display().to_string()),
        },
        sampler: Arc::new(Sampler::new(pid)),
        guard,
        log,
        core_instances,
        owned: plan.binary.is_some(),
    })
}

fn delta(before: Option<RegistryCounts>, after: Option<RegistryCounts>) -> Option<RegistryCounts> {
    let (before, after) = (before?, after?);
    Some(RegistryCounts {
        manifest_gets: after.manifest_gets.saturating_sub(before.manifest_gets),
        manifest_heads: after.manifest_heads.saturating_sub(before.manifest_heads),
        blob_gets: after.blob_gets.saturating_sub(before.blob_gets),
        not_found: after.not_found.saturating_sub(before.not_found),
    })
}

async fn run(args: Args) -> Result<u8> {
    let loud = args.report;
    let say = move |message: &str| {
        if loud {
            eprintln!("bench-scale: {message}");
        }
    };
    let machine = Machine::detect();
    let settings = resolve_settings(&args, &machine)?;
    if args.host_nats_only {
        if args.nats_url.is_none() {
            bail!("--host-nats-only attaches to a host on --nats-url; pass it");
        }
        if args.host_pid.is_none() && args.max_components.is_none() {
            bail!(
                "attached without --host-pid the bench cannot see memory, so nothing bounds the run; \
                 pass --host-pid or --max-components"
            );
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let run_id = format!("{:x}", now.as_millis());
    let workspace = provision::workspace_dir();
    let cache = args
        .cache_dir
        .clone()
        .unwrap_or_else(|| workspace.join("target/bench-scale"));
    let run_dir = args
        .run_dir
        .clone()
        .unwrap_or_else(|| cache.join("runs").join(now.as_secs().to_string()));
    std::fs::create_dir_all(&run_dir).with_context(|| format!("creating {}", run_dir.display()))?;

    // NATS: an existing server, or one of our own.
    let (_nats_server, nats_url) = match &args.nats_url {
        Some(url) => (None, url.clone()),
        None => {
            let binary = provision::ensure_nats(&cache, &args.nats_version, &say).await?;
            let (guard, url) = host::spawn_nats(&binary, &run_dir)?;
            (Some(guard), url)
        }
    };
    let nats = async_nats::ConnectOptions::new()
        .connect(&nats_url)
        .await
        .with_context(|| format!("connecting to {nats_url}"))?;

    // The component: an image the caller names, or ours, served locally.
    let (image, registry): (ImageInfo, Option<Registry>) = match &args.image {
        Some(reference) => (
            ImageInfo {
                reference: reference.clone(),
                source: "external".to_string(),
                size_bytes: None,
                layer_digest: None,
            },
            None,
        ),
        None => {
            let (wasm_path, source): (PathBuf, String) = match &args.wasm {
                Some(path) => (path.clone(), format!("wasm:{}", path.display())),
                None => (
                    provision::build_fixture(&workspace, &say)?,
                    format!("fixture:{}", provision::FIXTURE),
                ),
            };
            let wasm = std::fs::read(&wasm_path)
                .with_context(|| format!("reading {}", wasm_path.display()))?;
            let size_bytes = wasm.len() as u64;
            let image = Arc::new(Image::new("bench/http-handler", wasm)?);
            let registry = Registry::serve(Arc::clone(&image)).await?;
            (
                ImageInfo {
                    reference: image.reference(registry.addr),
                    source,
                    size_bytes: Some(size_bytes),
                    layer_digest: Some(image.layer_digest.clone()),
                },
                Some(registry),
            )
        }
    };

    // The host binary, unless attaching to one that is already running.
    let binary = match (&args.host_bin, args.host_nats_only) {
        (_, true) => None,
        (Some(path), false) => Some(path.clone()),
        (None, false) => {
            let version = args
                .wash_version
                .clone()
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
            Some(provision::ensure_wash(&cache, &version, &say).await?)
        }
    };
    let flags = match (&binary, args.host_cli) {
        (Some(binary), HostCli::Wash) => host::wash_host_flags(binary)?,
        _ => Default::default(),
    };

    if args.report {
        report::print_header(
            &machine,
            &settings,
            &image,
            &nats_url,
            &run_dir.display().to_string(),
        );
    }

    let flavours = match args.flavour {
        FlavourArg::Warm => vec![Flavour::Warm],
        FlavourArg::ScaleToZero => vec![Flavour::ScaleToZero],
        FlavourArg::Both => vec![Flavour::Warm, Flavour::ScaleToZero],
    };
    let plan = HostPlan {
        args: &args,
        nats: &nats,
        nats_url: &nats_url,
        binary: binary.as_deref(),
        flags: &flags,
        limits: HostLimits {
            memory_limit: settings.memory_limit_bytes,
            guest_memory: settings.memory_limit_bytes / 4 * 3,
            core_instances: settings.core_instances,
        },
        run_dir: &run_dir,
        run_id: &run_id,
    };
    let print_step = |step: &bench::Step, first: bool| {
        if loud {
            report::print_step(step, first);
        }
    };
    let run = Run {
        settings: &settings,
        image: &image.reference,
        run_id: &run_id,
        on_step: &print_step,
    };

    let mut reports = Vec::new();
    for flavour in flavours {
        // A fresh host per flavour, so neither inherits the other's footprint.
        let target = prepare_target(&plan, flavour).await?;
        if args.report {
            report::print_flavour_header(flavour, &target.info, target.sampler.source().describe());
        }
        let before = registry.as_ref().map(Registry::counts);
        let mut flavour_report = bench::run_flavour(&run, flavour, &target).await?;
        flavour_report.registry_requests = delta(before, registry.as_ref().map(Registry::counts));
        if args.report {
            report::print_flavour_summary(&flavour_report);
        }
        reports.push(flavour_report);
        target.shutdown();
    }

    let too_few = reports
        .iter()
        .any(|r| r.summary.served < settings.min_components);
    let exit_code = if too_few { EXIT_TOO_FEW } else { 0 };
    if args.report {
        report::print_comparison(&reports);
        if too_few {
            println!(
                "FAIL: a flavour served fewer than {} workloads",
                settings.min_components
            );
        }
    }
    if let Some(path) = &args.json {
        let report = Report {
            tool: "bench-scale",
            tool_version: env!("CARGO_PKG_VERSION"),
            started_unix_secs: now.as_secs(),
            machine,
            settings,
            image,
            nats_url,
            run_dir: run_dir.display().to_string(),
            flavours: reports,
            exit_code,
        };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(exit_code)
}

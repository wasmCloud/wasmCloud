# wasmCloud

[![Apache 2.0 License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/wasmcloud/wasmCloud)](https://github.com/wasmcloud/wasmCloud/releases)
[![Slack](https://img.shields.io/badge/Slack-wasmcloud-4A154B?logo=slack)](https://slack.wasmcloud.com/)

<p>
  <a href="https://www.cncf.io/projects/wasmcloud/">
    <img alt="CNCF Incubating" height="60" src="https://raw.githubusercontent.com/cncf/artwork/main/other/cncf-member/incubating/color/cncf-incubating-color.svg">
  </a>
</p>

**wasmCloud is a cloud native platform for running [WebAssembly](https://webassembly.org/) workloads across any cloud, Kubernetes, datacenter, or edge.**

Using wasmCloud, you can run microservices, functions, and agents as ultra-dense, deny-by-default bytecode sandboxes that are far more secure and efficient than traditional containers — *without* changing your operational model. Workloads are [WebAssembly components](https://wasmcloud.com/docs/overview/workloads/components) measured in kilobytes to low megabytes, starting in milliseconds, and portable across any conformant [WASI](https://wasi.dev/) runtime.

wasmCloud is a Cloud Native Computing Foundation [Incubating project](https://www.cncf.io/projects/wasmcloud/).

- **Documentation:** [wasmcloud.com/docs](https://wasmcloud.com/docs)
- **Quickstart:** [wasmcloud.com/docs/quickstart](https://wasmcloud.com/docs/quickstart)
- **Community:** [Slack](https://slack.wasmcloud.com/) · [Community meetings](https://wasmcloud.com/community/) · [GitHub Discussions](https://github.com/wasmCloud/wasmCloud/discussions)

## Why wasmCloud?

Containers default to **allow-by-default**: a container has broad access to the network, system calls, and environment variables unless something is explicitly blocked. Locking one down requires knowing everything it might try to do, then enforcing those restrictions from the outside.

WebAssembly components default to **deny-by-default**: a component can do nothing (no file I/O, network access, or system calls) unless a capability is explicitly granted. Capabilities are declared as language-agnostic [interfaces](https://wasmcloud.com/docs/overview/interfaces) in the component itself, so the security surface is small, visible, auditable, and enforced by the runtime rather than bolted on afterward.

wasmCloud runs WebAssembly components and manages their capabilities. You decide exactly which interfaces each component can access. Everything else is denied.

## What's in this repo

This is the wasmCloud monorepo. Other parts of the project such as documentation and language-specific resources for [TypeScript](https://github.com/wasmCloud/typescript) and [Go](https://github.com/wasmCloud/go) live in [separate repositories under the wasmCloud organization](https://github.com/wasmCloud).

| Path | Description |
| --- | --- |
| [`crates/wash`](./crates/wash/) | **Wasm Shell (`wash`)** — the CLI for scaffolding, building, and publishing WebAssembly components, and for running a local development host. |
| [`crates/wash-runtime`](./crates/wash-runtime/) | **`wash-runtime`** — the embeddable Rust runtime that powers `wash dev`, the cluster host, and custom embedded hosts. Wraps Wasmtime with a plugin-based capability model. |
| [`runtime-operator/`](./runtime-operator/) | **Runtime Operator** — Kubernetes operator that reconciles wasmCloud CRDs (`Host`, `Workload`, `WorkloadDeployment`, `WorkloadReplicaSet`, `Artifact`) and schedules workloads onto host pods via NATS. |
| [`runtime-gateway/`](./runtime-gateway/) | **Runtime Gateway** — HTTP gateway that proxies traffic to host pods. *Deprecated as of 2.0.3*; routing is now handled by the operator via EndpointSlices on standard Kubernetes Services. The chart still installs the gateway by default for backwards compatibility (set `gateway.enabled: false` to skip). |
| [`charts/runtime-operator/`](./charts/runtime-operator/) | Helm chart for installing the operator, host runtime, and (optionally) NATS as a single release. |
| [`proto/`](./proto/) | Protobuf definitions for control-plane messages exchanged between the operator and hosts over NATS. |
| [`templates/`](./templates/) | Rust project templates consumed by `wash new` (`http-hello-world`, `http-handler`, `http-kv-handler`, `service-tcp`, etc.). |
| [`examples/`](./examples/) | Reference component projects (`blobby`, `grpc-hello-world`, `otel-config`, `qrcode`, persistent-storage variants). Built and pushed to `ghcr.io/wasmcloud/components/*` by CI. |
| [`deploy/`](./deploy/) | `kind` and `k3s` configurations for local clusters. |
| [`wit/`](./wit/) | Top-level WIT definitions shared across the project (messaging, secrets). |

## Install

### Wasm Shell (`wash`)

**macOS / Linux:**

```bash
curl -fsSL https://wasmcloud.com/sh | bash
```

**Windows (PowerShell):**

```powershell
iwr -useb https://wasmcloud.com/ps1 | iex
```

**Homebrew:**

```bash
brew install wasmcloud/wasmcloud/wash
```

**winget:**

```powershell
winget install wasmCloud.wash
```

**From source:**

```bash
git clone https://github.com/wasmcloud/wasmCloud.git
cd wasmCloud
cargo install --path crates/wash
```

Verify:

```bash
wash -V
```

For details and options, see the [Installation guide](https://wasmcloud.com/docs/installation).

### wasmCloud on Kubernetes

Install the operator (and a bundled NATS) from the OCI Helm chart, applying the recommended overlay that disables the deprecated Runtime Gateway and routes HTTP via standard Kubernetes Services:

```bash
helm install wasmcloud oci://ghcr.io/wasmcloud/charts/runtime-operator \
  --namespace wasmcloud --create-namespace \
  -f https://raw.githubusercontent.com/wasmCloud/wasmCloud/refs/heads/main/charts/runtime-operator/values.local.yaml
```

For a local `kind` cluster, the deploy assets and full walkthrough live at [wasmcloud.com/docs/installation](https://wasmcloud.com/docs/installation#install-wasmcloud-on-kubernetes).

## Quickstart

Requires the [Rust toolchain](https://www.rust-lang.org/tools/install) and `rustup target add wasm32-wasip2`.

```bash
# Scaffold a new component
wash new https://github.com/wasmCloud/wasmCloud.git \
  --subfolder templates/http-hello-world \
  --name hello
cd hello

# Run in a hot-reload development loop
wash dev
```

In another terminal:

```bash
curl localhost:8000
# Hello from wasmCloud!
```

For a full walkthrough (component development, persistent storage, Kubernetes deployment), see [wasmcloud.com/docs/quickstart](https://wasmcloud.com/docs/quickstart).

## The platform

The wasmCloud platform has three primary parts, all developed in this repository:

- **[Wasm Shell (`wash`) CLI](https://wasmcloud.com/docs/wash)** — develop and publish components from any language that targets WASI Preview 2 (Rust, Go, TypeScript, Python, and more).
- **[Runtime (`wash-runtime`)](https://wasmcloud.com/docs/runtime)** — the embeddable Rust runtime and host API. Use it via `wash dev`, run it as a cluster host managed by the operator, or [build a custom host](https://wasmcloud.com/docs/runtime/building-custom-hosts) for embedded and edge scenarios.
- **[Kubernetes Operator (`runtime-operator`)](https://wasmcloud.com/docs/kubernetes-operator)** — runs wasmCloud infrastructure as standard Kubernetes resources. Auto-scaling, observability, GitOps, and RBAC all work through your existing tooling.

The runtime exposes capabilities through three mechanisms:

- **Built-in via `wasmtime-wasi`** — `wasi:filesystem`, `wasi:clocks`, `wasi:random`, `wasi:io`, `wasi:sockets`, `wasi:cli`.
- **HTTP handler** (`HttpServer`) — `wasi:http` (client and server).
- **Host plugins** (`with_plugin()`, feature-flagged in-memory and NATS-backed variants) — `wasi:keyvalue`, `wasi:blobstore`, `wasi:config`, `wasi:logging`, `wasmcloud:messaging`.

Hosts can be extended with additional custom plugins at build time. See [Creating Host Plugins](https://wasmcloud.com/docs/runtime/creating-host-plugins).

## `wash` commands

| Command | Description |
| --- | --- |
| `wash build` | Build a Wasm component using the language toolchain configured in `.wash/config.yaml`. |
| `wash completion` | Generate shell completion scripts (bash, zsh, fish, PowerShell). |
| `wash config` | View and manage `wash` configuration. |
| `wash dev` | Hot-reload development loop with an embedded host. |
| `wash host` | Run a cluster host (`washlet`) that surfaces the `wash-runtime` API over NATS. |
| `wash new` | Scaffold a new project from a git repository or local subfolder. |
| `wash oci` | Push or pull Wasm components to/from an OCI registry. |
| `wash update` | Self-update `wash` to the latest release. |
| `wash wit` | Manage WIT dependencies. |

Run `wash --help` or `wash help <command>` for detailed usage.

### Shell completion

<details>
<summary>Zsh</summary>

```shell
mkdir -p ~/.zsh/completion
wash completion zsh > ~/.zsh/completion/_wash
```

Add to `~/.zshrc`:

```shell
fpath=(~/.zsh/completion $fpath)
autoload -Uz compinit && compinit
```

</details>

<details>
<summary>Bash</summary>

```shell
. <(wash completion bash)
```

</details>

<details>
<summary>Fish</summary>

```shell
mkdir -p ~/.config/fish/completions
wash completion fish > ~/.config/fish/completions/wash.fish
```

</details>

<details>
<summary>PowerShell</summary>

```powershell
wash completion powershell > $env:UserProfile\Documents\WindowsPowerShell\Scripts\wash.ps1
```

</details>

## Building from source

This is a Cargo workspace targeting Rust `1.91.0+` (edition 2024) for the Rust crates and Go `1.26.0` for the operator and gateway.

```bash
# Build the default workspace members (wash CLI by default)
cargo build

# Build everything
cargo build --workspace
```

The `wash-runtime` integration tests and benchmarks load precompiled wasm fixtures. You can build the
fixtures with the  `xtask` runner.

> [!WARNING]
> As the current version of `wasm-component-ld` that is in use in upstream Rust is
> older and does not support certain Component Model features that wasmCloud does,
> you may have to install `wasm-component-ld`:
>
> ```console
> cargo install wasm-component-ld
> ```
> (consider also using `cargo binstall` if you have it installed)
>
> Once you have `wasm-component-ld` installed (any version greater than 0.5.24),
> you can convince `cargo` to use it by settting the following environment variable
> ```console
> export CARGO_TARGET_WASM32_WASIP2_LINKER=$HOME/.cargo/bin/wasm-component-ld
> ```

```bash
# export CARGO_TARGET_WASM32_WASIP2_LINKER=$HOME/.cargo/bin/wasm-component-ld
cargo xtask build-fixtures
cargo test
```
(NOTE: you do not have to use a modified linker for anything other than building fixtures)

Remember to rebuild the fixtures if you change any code `crates/wash-runtime/tests/fixtures/`.

For Go components (operator, gateway), see their respective `README.md` files and `make` targets.

See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for code conventions, error handling expectations, and the PR process.

## Releases

Releases ship every two weeks: each Tuesday at 16:00 UTC on the train's cycle, the next `vX.Y.Z` is cut from `main` automatically. Anything merged before the train leaves ships in that release. See [`RELEASE_RUNBOOK.md`](./RELEASE_RUNBOOK.md) for the full cadence and procedure.

## Project documentation

- [`CONTRIBUTING.md`](./CONTRIBUTING.md) — how to contribute, code style, and PR conventions
- [`CONTRIBUTION_LADDER.md`](./CONTRIBUTION_LADDER.md) — contributor → maintainer progression
- [`GOVERNANCE.md`](./GOVERNANCE.md) — project governance and decision-making
- [`MAINTAINERS.md`](./MAINTAINERS.md) — current maintainers by area
- [`SECURITY.md`](./SECURITY.md) — vulnerability reporting and security policy
- [`ROADMAP.md`](./ROADMAP.md) — quarterly roadmap process
- [`RELEASE_RUNBOOK.md`](./RELEASE_RUNBOOK.md) — release cadence and runbook

## Community

- [**Slack**](https://slack.wasmcloud.com/) — the primary place for real-time discussion
- [**Community meetings**](https://wasmcloud.com/community/) — weekly, recorded, all welcome
- [**GitHub Discussions**](https://github.com/wasmCloud/wasmCloud/discussions) — long-form questions and roadmap input
- [**Issues**](https://github.com/wasmCloud/wasmCloud/issues) — bug reports and feature requests (the [`good-first-issue` label](https://github.com/wasmCloud/wasmCloud/issues?q=is%3Aopen+is%3Aissue+label%3A%22good+first+issue%22) is a good place to start)
- [**Security**](./SECURITY.md) — report vulnerabilities privately to `security@wasmcloud.com`

## Further reading

- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [WASI Preview 2](https://github.com/WebAssembly/WASI/blob/main/docs/Preview2.md)
- [Wasmtime](https://wasmtime.dev/)
- [NATS](https://nats.io/)

## License

This project is licensed under the Apache License 2.0 — see the [LICENSE](LICENSE) file for details.

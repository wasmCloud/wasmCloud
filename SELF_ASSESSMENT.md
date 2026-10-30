# wasmCloud Security Self-Assessment

This is the wasmCloud project's security self-assessment, written by its maintainers using the [OpenSSF self-assessment template](https://security-assessments.openssf.org/). It covers this repository. What wasmCloud defends against, and what it treats as a vulnerability, are in the [threat model](THREAT_MODEL.md); this document points to it rather than repeating it.

## Table of contents

- [Metadata](#metadata)
  - [Security links](#security-links)
- [Overview](#overview)
  - [Background](#background)
  - [Actors](#actors)
  - [Actions](#actions)
  - [Goals](#goals)
  - [Non-goals](#non-goals)
- [Self-assessment use](#self-assessment-use)
- [Security functions and features](#security-functions-and-features)
- [Project compliance](#project-compliance)
- [Secure development practices](#secure-development-practices)
- [Security issue resolution](#security-issue-resolution)
- [Appendix](#appendix)

## Metadata

| | |
| --- | --- |
| Assessment Stage | Complete (2026-10-27) |
| Software | <https://github.com/wasmCloud/wasmCloud> |
| Security Provider | No. wasmCloud runs untrusted WebAssembly components in isolation, but its primary function is running applications, not securing other systems. |
| Languages | Rust (the `wash` CLI, the `wash-runtime` host, `runtime-gateway`), Go (`runtime-operator`), Helm templates |
| SBOM | Dependencies are pinned in [`Cargo.lock`](Cargo.lock) and the Go modules' `go.sum` files. Releases do not yet publish a separate SBOM. |

### Security links

| Doc | URL |
| --- | --- |
| Security policy and vulnerability reporting | [SECURITY.md](SECURITY.md) |
| Threat model | [THREAT_MODEL.md](THREAT_MODEL.md) |
| Host threat assessment (Gemara) | [capability catalog](docs/security/capability-catalog.yaml) and [threat catalog](docs/security/threat-catalog.yaml) |
| Security Insights | [security-insights.yml](security-insights.yml) |
| Release process and artifact verification | [RELEASE_RUNBOOK.md](RELEASE_RUNBOOK.md) |
| Host security flags and their effects | [THREAT_MODEL.md: Operator settings that weaken a goal](THREAT_MODEL.md#operator-settings-that-weaken-a-goal) |
| Helm chart values | [charts/runtime-operator/values.yaml](charts/runtime-operator/values.yaml) |
| Third-party audit (2023, wasmCloud 1.x) | [OSTIF and Trail of Bits report](https://ostif.org/wp-content/uploads/2023/10/wasmcloud-audit-ostif-trail-of-bits-final.pdf) |

## Overview

wasmCloud runs WebAssembly components as sandboxed, deny-by-default workloads on Kubernetes or on a single machine, so teams can deploy microservices, functions and agents without changing how they operate their clusters.

### Background

A [WebAssembly component](https://component-model.bytecodealliance.org/) is a portable binary that can only call the functions it imports. A host decides which imports to provide, so a component has no access to files, the network or other components unless the host links an interface that gives it that access. wasmCloud's host uses [wasmtime](https://wasmtime.dev/) to compile and run components, and implements [WASI](https://wasi.dev/) and wasmCloud's own interfaces for HTTP, messaging, key-value storage, secrets and more.

A **workload** is a set of components, and optionally one long-running service component, that one spec describes and that are deployed together. The spec lists what the workload is granted: the hosts it may reach, the volumes it mounts, its configuration and secrets, and the interfaces it imports. On Kubernetes, `runtime-operator` turns `Workload` resources into those specs and schedules them onto hosts over NATS. `runtime-gateway` routes inbound HTTP to the hosts serving each workload.

### Actors

The [threat model's actor table](THREAT_MODEL.md#actors-and-trust) lists who is trusted and why. The isolated parts of the system are:

| Actor | Role | Isolation |
| --- | --- | --- |
| Component | Untrusted application code. | Each runs in its own wasmtime instance with its own linear memory. It reaches only the imports its workload's spec grants. |
| Workload | A set of components from one spec, treated as one tenant. | Its own stores, memory and virtual network. `127.0.0.1` inside a workload reaches only that workload. |
| Host (`wash host`, `wash-runtime`) | Runs workloads, enforces their grants, and implements host interfaces. | A separate process, normally a Kubernetes pod with the chart's hardening: non-root, read-only root filesystem, all capabilities dropped, `RuntimeDefault` seccomp, no service account token. |
| Plugin | Implements an interface for workloads. Native plugins are host code; Wasm host component plugins are sandboxed components. | A Wasm plugin runs in its own instance and serves every workload that binds it. The host tells it which workload is calling. |
| `runtime-operator` | Reconciles `Workload` and host resources and schedules workloads onto hosts. | A separate pod with its own Kubernetes RBAC. It reaches hosts only over NATS. |
| `runtime-gateway` | Routes inbound HTTP to hosts. | A separate pod. It forwards requests; it doesn't run components. |
| NATS | The control-plane bus between the operator and hosts. | Separate deployment. Anyone who can publish on `runtime.host.>` can start any workload, so operators secure it with TLS and credentials. |

### Actions

**Starting a workload.** The operator (or anyone with control-plane NATS credentials) sends a host a workload spec. The host pulls each component from its OCI registry, validates and compiles it with wasmtime, and refuses binaries that fail validation. It links only the interfaces the spec grants, applies the spec's `allowedHosts`, volumes, configuration and secrets, and checks each `hostPath` volume against the operator's `--allowed-host-path` directories and reserved paths. The workload's memory, connection and file-descriptor limits apply from its first instruction.

**Serving a request.** `runtime-gateway` receives an HTTP request and forwards it to a host serving a workload that declared a route for that hostname. The host parses the request, picks an instance of the workload's component, and calls its exported handler. The component sees only the request; it cannot see other workloads' traffic.

**Outbound calls.** When a component makes an HTTP request, the host checks the destination name against the workload's `allowedHosts`. When it opens a raw socket, the host checks the resolved address against `allowedHosts` and the host's address policy, which by default refuses loopback, link-local (including cloud metadata) and multicast. Connections count against the workload's and the host's connection limits.

**Calling a plugin.** When a component calls an interface a plugin implements, the host passes the plugin the calling workload's identity. The guest cannot set or forge it, and the plugin authorizes on it.

**Releasing.** A release train opens a version-bump PR every two weeks. After a maintainer approves it and CI passes, the tag is pushed. Container images that passed CI as canaries are promoted to the release tag without a rebuild, and binaries, images and WIT packages get Sigstore-signed SLSA provenance. See [RELEASE_RUNBOOK.md](RELEASE_RUNBOOK.md).

### Goals

The [threat model's security goals](THREAT_MODEL.md#security-goals) hold for a host with default settings:

1. **Sandbox.** A component runs only as WebAssembly in its own instance and cannot crash or corrupt the host.
2. **Grants.** A component reaches only the hosts, addresses, files, config, secrets and interfaces its spec grants.
3. **Isolation.** A workload cannot read or modify another workload's memory, files, connections or state unless they are explicitly shared.
4. **Host.** A workload cannot reach the host's own credentials, configuration, kernel filesystems or listeners.
5. **Egress.** Raw socket egress reaches only addresses the spec and the address policy permit.
6. **Resources.** One workload cannot exhaust the host's memory, connections or file descriptors beyond operator-set limits.
7. **Untrusted input.** No request, response, message or registry artifact from a peer can crash the host or corrupt its memory.

### Non-goals

- **Deciding who may author a workload spec.** A spec is a set of grants, and the host trusts it. Platforms that let tenants write specs constrain them with Kubernetes RBAC and admission policy.
- **Verifying component signatures.** The host sandboxes components whatever their origin. Which publishers to trust is the operator's decision.
- **Protecting a component from itself.** A component that traps or computes the wrong answer harms only itself.
- **Side channels.** Spectre-class and timing side channels between workloads on one host are not mitigated. Mutually hostile tenants belong on separate hosts or nodes.
- **Fair CPU sharing.** The host does not cap a workload's share of CPU; pod CPU limits bound the host.
- **Developer workflows.** `wash dev` runs a developer's own code on the developer's own machine and is out of scope.

The threat model's [known limitations](THREAT_MODEL.md#known-limitations) and [operator responsibilities](THREAT_MODEL.md#operator-responsibilities) list the rest.

## Self-assessment use

This self-assessment is created by the wasmCloud team to perform an internal analysis of the project's security. It is not intended to provide a security audit of wasmCloud, or function as an independent assessment or attestation of wasmCloud's security health.

This document serves to provide wasmCloud users with an initial understanding of wasmCloud's security, where to find existing security documentation, wasmCloud plans for security, and general overview of wasmCloud security practices, both for development of wasmCloud as well as security of wasmCloud.

This document provides readers with an initial understanding of wasmCloud to assist in a joint-assessment, necessary for projects under incubation. Taken together, this document and the joint-assessment serve as a cornerstone for if and when wasmCloud seeks graduation and is preparing for a security audit.

## Security functions and features

### Critical

These are design elements that make wasmCloud secure and are not configurable.

- **wasmtime sandbox.** Every component runs as WebAssembly in its own instance, with no way to run native code or touch memory outside its own.
- **Capability linking.** A component gets only the imports its spec grants. Nothing is ambient.
- **Workload isolation.** Separate stores, memory and virtual network per workload. Workloads reach each other only through interfaces the host links between them.
- **Host-asserted identity.** The host tells plugins which workload is calling, and the guest cannot forge it.
- **`wasi:http` egress check.** HTTP requests to hosts outside `allowedHosts` are refused in every mode.
- **Host loopback.** The machine's own loopback needs both `--allow-host-loopback` and a grant in the spec, and never reaches a port the host owns.
- **No inbound listening.** A component cannot accept connections. A service listens only on its workload's virtual network, and outside it only on ports its spec publishes.
- **`hostPath` gate.** A `hostPath` volume must lie inside an `--allowed-host-path` directory and may never expose a reserved path, a kernel filesystem or another workload's scratch volume.
- **Connection limits.** Per-workload and host-wide connection limits are always enforced.

### Security relevant

These are settings that strengthen or weaken a deployment. The threat model's [settings table](THREAT_MODEL.md#operator-settings-that-weaken-a-goal) lists each one's effect.

- **Address policy.** `--deny-special-ranges` (on by default) refuses loopback, link-local and multicast egress. `--deny-private-ranges` also refuses RFC 1918, ULA and CGNAT addresses.
- **Enforce and count modes.** `--socket-egress` and `--guest-memory-mode` enforce by default. `count` reports what would have been refused without refusing it, for migrations.
- **Memory limits.** `--max-guest-memory` and per-workload memory limits.
- **Reserved paths.** `--reserved-host-path` names credential files no workload may mount, in addition to the files the host's own flags name.
- **Control-plane TLS.** The host's NATS connections take a CA, client certificate and key.
- **Helm chart hardening.** Host pods run non-root with a read-only root filesystem, all capabilities dropped, the `RuntimeDefault` seccomp profile and no service account token.
- **Secrets.** Workloads read secrets through the `wasmcloud:secrets` interface, and each workload reaches only the secrets its spec binds.
- **Relaxations.** `--http-local-routing`, `--wasi-webgpu` and `--allow-insecure-registries` each weaken a goal and are off by default.

## Project compliance

wasmCloud is not documented as meeting any regulatory or industry standard such as PCI-DSS, ISO 27001 or SOC 2. The project publishes an [OpenSSF Security Insights](security-insights.yml) file and is working toward [OSPS Baseline](https://baseline.openssf.org/) Level 2.

## Secure development practices

### Development pipeline

- **Contributions.** Every commit carries a [DCO](https://developercertificate.org/) `Signed-off-by` line, checked on each pull request. Commits are not required to be cryptographically signed.
- **Review.** A ruleset on `main` requires a pull request with at least one approval, a code owner's review, and approval of the most recent push by someone other than its author. Stale approvals are dismissed on new pushes. Force pushes and deletion of `main` are blocked, and history must be linear.
- **Required checks.** A `gate` job aggregates the CI for whatever a pull request changes, including build, tests, `clippy` and `rustfmt`, and must pass before merge.
- **Automated security checks.** CodeQL (Rust and Go, weekly), `cargo-audit` and a `cargo-deny` license check (daily, on pushes to `main`, and on dependency changes), Dependabot alerts and security updates, zizmor for GitHub Actions workflows, Trivy and Checkov for the Helm chart, a weekly OpenSSF Scorecard run, and an OSPS Baseline scan on every push to `main` and weekly. [security-insights.yml](security-insights.yml) lists each with where its results are.
- **Secrets.** GitHub secret scanning and push protection are on.
- **Workflows.** GitHub Actions are pinned by commit SHA, and Dependabot keeps them current.
- **Releases.** Releases are cut by an automated train, with a maintainer's approval. Images that passed CI are promoted without a rebuild. Release binaries, container images and WIT packages carry Sigstore-signed SLSA provenance, verifiable with `gh attestation verify --repo wasmCloud/wasmCloud`. Release tags are immutable.
- **Privileged workflows.** `release-tag.yml` holds release credentials: write access to contents and packages, OIDC for signing, and a bot token that pushes tags and triggers the Homebrew, winget and docs updates. It never runs code from an untrusted contributor (OSPS-BR-01.03):
  - It starts only after `release-train`, which runs on a schedule or a maintainer's dispatch, or from a maintainer's dispatch of its own. No pull request can trigger either.
  - It checks out `main`, or the SHA a maintainer dispatched with, and never a ref from the triggering run.
  - It takes binaries from `wash.yml` runs on the release commit, where the binary builds run only on `main` pushes, dispatches, the merge queue, and pull requests that edit `wash.yml`. The commit SHA fixes the code those runs built.
  - It promotes `sha-<commit>` canary images, which only `main` pushes and dispatches publish. Pull request runs can't write to GHCR or read `main`'s build caches.
  - Dispatch inputs and values from the triggering run reach its scripts through `env:`, not inline expressions, and it runs on GitHub-hosted runners.

### Communication channels

- **Internal.** Maintainers use the `#maintainers` channel in the [wasmCloud Slack](https://wasmcloud.slack.com/), the `cncf-wasmCloud-maintainers@lists.cncf.io` mailing list, and GitHub. Security reports are handled privately by the security team described in [SECURITY.md](SECURITY.md#security-team-membership).
- **Inbound.** Users file [GitHub issues](https://github.com/wasmCloud/wasmCloud/issues) and [Discussions](https://github.com/wasmCloud/wasmCloud/discussions), ask in [Slack](https://slack.wasmcloud.com/), and join the [community meetings](https://wasmcloud.com/community/). Vulnerabilities go to security@wasmcloud.com or [GitHub private vulnerability reporting](https://github.com/wasmCloud/wasmCloud/security/advisories/new).
- **Outbound.** [GitHub Releases](https://github.com/wasmCloud/wasmCloud/releases) with release notes, [GitHub Security Advisories](https://github.com/wasmCloud/wasmCloud/security/advisories), the [wasmCloud blog](https://wasmcloud.com/blog), Slack, and community meetings.

### Ecosystem

wasmCloud is a CNCF incubating project. It runs on Kubernetes through `runtime-operator`, uses NATS for its control plane, and pulls components and plugins as OCI artifacts from any OCI registry. It is built on wasmtime and the WebAssembly component model from the Bytecode Alliance, and implements WASI, so components built for wasmCloud run on other conformant WASI runtimes. Hosts export telemetry over OpenTelemetry.

## Security issue resolution

### Responsible disclosure process

Reporters email security@wasmcloud.com or use GitHub private vulnerability reporting on the affected repository. The [threat model](THREAT_MODEL.md#what-we-treat-as-a-vulnerability) says what is and isn't a vulnerability. Details stay within the security team and the maintainers fixing the issue, shared on a need-to-know basis, and reporters are asked not to disclose until a fix is released. Reporters are credited with their permission.

### Vulnerability response process

The security team, a subset of active maintainers, receives each report and acknowledges it within 3 business days. For each accepted report, a patch team that includes the affected component's maintainers fixes it and prepares a release. The reporter is kept informed through triage, fix and release. See [SECURITY.md](SECURITY.md#security-vulnerability-response).

### Incident response

The patch team confirms and triages a report, then fixes it. The fix ships in a patch release, which the release train allows at any time outside the two-week cadence. The GitHub Security Advisory is published the same day, after the release, with the affected versions, the fix and any workaround, and release notes link it. A CVE is requested for each advisory; the advisory may be published before the CVE ID arrives and is updated when it does. Fixes in wasmtime and other dependencies reach users through a wasmCloud release.

## Appendix

### Known issues over time

- [GHSA-2cmx-rr54-88g5](https://github.com/wasmCloud/wasmcloud-otp/security/advisories/GHSA-2cmx-rr54-88g5) (CVE-2022-21707, medium, January 2022): wasmCloud 0.x actors without signed capabilities could receive unauthorized invocations. It affected the archived Elixir host, `wasmcloud-otp`.
- No advisories have been published for this repository's 2.x code.
- In October 2023, OSTIF and Trail of Bits audited wasmCloud 1.x. Their [report](https://ostif.org/wp-content/uploads/2023/10/wasmcloud-audit-ostif-trail-of-bits-final.pdf) covers the findings and fixes. wasmCloud 2.x replaced that host, so its findings don't map directly onto this code.

### OpenSSF Best Practices

wasmCloud has held the [OpenSSF Best Practices](https://www.bestpractices.dev/projects/6363) passing badge since November 2023. The entry was last updated then, and is 64% of the way to silver. Its OSPS Baseline levels are not filled in yet.

### Case studies

- **Multi-tenant HTTP services on Kubernetes.** A platform team runs many teams' HTTP services as components on shared host pods. Each team's workload is isolated from the others, egress is limited to the hosts its spec lists, and the platform restricts who may create `Workload` resources with RBAC and admission policy.
- **Event-driven processing.** Components subscribe to NATS subjects or JetStream consumers and process messages.
- **Edge and resource-constrained hosts.** Components measured in kilobytes to low megabytes start in milliseconds, so many workloads can share one small host.

### Related projects

- **Containers on Kubernetes.** A container shares the node's kernel and starts with whatever the image contains. A wasmCloud component starts with nothing and can only call the interfaces it is granted, and the same component runs on any architecture the host supports.
- **wasmtime.** wasmtime is the runtime wasmCloud embeds. wasmCloud adds what wasmtime leaves to embedders: scheduling across hosts, grants and policy per workload, networking, plugins and Kubernetes integration.

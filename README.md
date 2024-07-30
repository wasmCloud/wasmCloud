[![Stars](https://img.shields.io/github/stars/wasmcloud?color=gold&label=wasmCloud%20Org%20Stars)](https://github.com/wasmcloud/)
[![Homepage and Documentation](https://img.shields.io/website?label=Documentation&url=https%3A%2F%2Fwasmcloud.com)](https://wasmcloud.com)
[![CNCF sandbox project](https://img.shields.io/website?label=CNCF%20Sandbox%20Project&url=https://landscape.cncf.io/?selected=wasm-cloud&item=orchestration-management--scheduling-orchestration--wasmcloud)](https://landscape.cncf.io/?selected=wasm-cloud&item=orchestration-management--scheduling-orchestration--wasmcloud)
![Powered by WebAssembly](https://img.shields.io/badge/powered%20by-WebAssembly-orange.svg)
[![OpenSSF Best Practices](https://bestpractices.coreinfrastructure.org/projects/6363/badge)](https://bestpractices.coreinfrastructure.org/projects/6363)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/wasmcloud/wasmcloud/badge)](https://securityscorecards.dev/viewer/?uri=github.com/wasmcloud/wasmcloud)
[![Artifact Hub](https://img.shields.io/endpoint?url=https://artifacthub.io/badge/repository/wasmcloud-chart)](https://artifacthub.io/packages/search?repo=wasmcloud-chart)
[![CLOMonitor](https://img.shields.io/endpoint?url=https://clomonitor.io/api/projects/cncf/wasm-cloud/badge)](https://clomonitor.io/projects/cncf/wasm-cloud)
[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B40030%2Fgit%40github.com%3AwasmCloud%2FwasmCloud.git.svg?type=small)](https://app.fossa.com/projects/custom%2B40030%2Fgit%40github.com%3AwasmCloud%2FwasmCloud.git?ref=badge_small)
[![twitter](https://img.shields.io/twitter/follow/wasmcloud?style=social)](https://twitter.com/wasmcloud)
[![youtube subscribers](https://img.shields.io/youtube/channel/subscribers/UCmZVIWGxkudizD1Z1and5JA?style=social)](https://youtube.com/wasmcloud)
[![youtube views](https://img.shields.io/youtube/channel/views/UCmZVIWGxkudizD1Z1and5JA?style=social)](https://youtube.com/wasmcloud)

![wasmCloud logo](https://raw.githubusercontent.com/wasmCloud/branding/main/02.Horizontal%20Version/Pixel/PNG/Wasmcloud.Logo-Hrztl_Color.png)

wasmCloud is an open source Cloud Native Computing Foundation (CNCF) project that enables teams to build, manage, and scale polyglot Wasm apps across any cloud, K8s, or edge.

wasmCloud offers faster development cycles with reusable, polyglot components and centrally maintainable apps, allowing platform teams to manage thousands of diverse applications. It integrates seamlessly with existing stacks like Kubernetes and cloud providers, while providing portability across different operating systems and architectures without new builds. With custom capabilities, scale-to-zero, fault-tolerant features, and deployment across clouds, wasmCloud enables secure, reliable, and scalable applications without vendor lock-in.

# Getting Started

## Installation

Install the wasmCloud Shell (`wash`) using the [installation guide](https://wasmcloud.com/docs/installation).

## Walkthrough

If you're new to the wasmCloud ecosystem, a great place to start is the [getting started](https://wasmcloud.com/docs/getting-started/) walkthrough.

## Quickstart

The following commands launch wasmCloud in a local development environment and deploy a simple "hello world" WebAssembly component, written in Rust, Go, TypeScript, or Python.

```console
wash up -d
wash new component helloworld
wash build -p ./helloworld
wash app deploy ./helloworld/wadm.yaml
curl localhost:8080
```

## Features

1. [Declarative WebAssembly Orchestration](https://wasmcloud.com/docs/concepts/applications)
2. [Seamless Distributed Networking](https://wasmcloud.com/docs/concepts/lattice)
3. [Vendorless Application Components](https://wasmcloud.com/docs/concepts/components#application-components)
4. [Completely OTEL Observable](https://wasmcloud.com/docs/category/observability)
5. [Defense-In-Depth Security By Default](https://wasmcloud.com/docs/category/security)

## Examples

### 👟 Runnable examples

Want to get something running quickly? Check out the [`examples` directory of this repository](./examples). Examples are organized by programming language so you can easily find samples in your language of choice.

# 🗺️ Roadmap and Vision

wasmCloud is a community-led project and plans quarterly roadmaps in community meetings. Please check out the [latest roadmap](https://wasmcloud.com/docs/roadmap) for more information, and the [wasmCloud Roadmap project](https://github.com/orgs/wasmCloud/projects/7/views/11) to see the status of new features, improvements, bug fixes, and documentation.

## Releases

The latest release and changelog can be found on the [releases page](https://github.com/wasmCloud/wasmCloud/releases).

# 🧑‍💻 Contributing

Want to get involved? For more information on how to contribute and our contributor guidelines, check out the [contributing readme](./CONTRIBUTING.md).

---

## 🌇 Community Resources

### Community Meetings

We host weekly community meetings at 1pm EST on Wednesdays. These community meetings are livestreamed to our Twitter account and to [YouTube](https://www.youtube.com/@wasmCloud/streams). You can find the agenda and notes for each meeting in the [community](https://wasmcloud.com/community) secton of our webste. If you're interested in joining in on the call to demo or take part in the discussion, we have a Zoom link on our [community calendar](https://calendar.google.com/calendar/u/0/embed?src=c_6cm5hud8evuns4pe5ggu3h9qrs@group.calendar.google.com).

### Slack

We host our own [community slack](https://slack.wasmcloud.com) for all community members to join and talk about WebAssembly, wasmCloud, or just general cloud native technology. For those of you who are already on the [CNCF Slack](https://cloud-native.slack.com/), we also have our own channel at [#wasmcloud](https://cloud-native.slack.com/archives/C027YTXEYFL).

---

## 📚 Reference Documentation

wasmCloud uses some terminology you might not be familiar with. Check out the [platform overview](https://wasmcloud.com/docs/concepts/) section of our docs for a deeper dive.

---

## RPC Framework (wRPC)

wasmCloud uses [wRPC](https://github.com/bytecodealliance/wrpc), [Component-native](https://component-model.bytecodealliance.org/) transport-agnostic RPC protocol and framework based on [WebAssembly Interface Types (WIT)](https://component-model.bytecodealliance.org/design/wit.html) to enable seamless communication among the host runtime, components, and providers. wRPC is a [Bytecode Alliance](https://bytecodealliance.org/) hosted project.

---

## Wasm-native Orchestration & Declarative Deployments

The **w**asmCloud **A**pplication **D**eployment **M**anager [wadm](https://github.com/wasmCloud/wadm) is a Wasm-native orchestrator for managing and scaling declarative wasmCloud applications. Applications are defined using the [Open Application Model](https://oam.dev/) format.

---

## Language Support & SDKs

wasmCloud is compatible with any language that supports the [WebAssembly Component Model](https://component-model.bytecodealliance.org/language-support.html). We provide first-party examples in [Rust](./examples/rust/), [Go](./examples/golang/), [Python](./examples/python), and [TypeScript](./examples/typescript/). If your language isn't listed yet, let us know with the [language support form](https://share.hsforms.com/1cedPVcwwQd6dQePZ3BWccQccyup).

### Capability Provider SDK

wasmCloud provides the following SDKs for creating capability providers; native executable host plugins for extending wasmCloud with custom implementations or custom capabilities:

1. [Rust provider-sdk](./crates/provider-sdk), with a [custom template provider](./examples/rust/providers/custom-template/) built for getting started quickly
1. [Golang provider-sdk-go](https://github.com/wasmCloud/provider-sdk-go), with a [custom template provider](./examples/golang/providers/custom-template/) built for getting started quickly

---

_We are a Cloud Native Computing Foundation [sandbox project](https://www.cncf.io/sandbox-projects/)._

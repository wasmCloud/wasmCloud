[![Documentation](https://img.shields.io/website?label=Documentation&url=https%3A%2F%2Fwasmcloud.com)](https://wasmcloud.com)
[![CNCF sandbox project](https://img.shields.io/website?label=CNCF%20Sandbox%20Project&url=https://landscape.cncf.io/?selected=wasm-cloud)](https://landscape.cncf.io/?selected=wasm-cloud)
[![Stars](https://img.shields.io/github/stars/wasmcloud?color=gold&label=wasmCloud%20Org%20Stars)](https://github.com/wasmcloud/)
![Powered by WebAssembly](https://img.shields.io/badge/powered%20by-WebAssembly-orange.svg)<br />
[![OpenSSF Best Practices](https://bestpractices.coreinfrastructure.org/projects/6363/badge)](https://bestpractices.coreinfrastructure.org/projects/6363)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/wasmcloud/wasmcloud/badge)](https://securityscorecards.dev/viewer/?uri=github.com/wasmcloud/wasmcloud)
[![Artifact Hub](https://img.shields.io/endpoint?url=https://artifacthub.io/badge/repository/wasmcloud-chart)](https://artifacthub.io/packages/search?repo=wasmcloud-chart)
[![CLOMonitor](https://img.shields.io/endpoint?url=https://clomonitor.io/api/projects/cncf/wasm-cloud/badge)](https://clomonitor.io/projects/cncf/wasm-cloud)
[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B40030%2Fgit%40github.com%3AwasmCloud%2FwasmCloud.git.svg?type=small)](https://app.fossa.com/projects/custom%2B40030%2Fgit%40github.com%3AwasmCloud%2FwasmCloud.git?ref=badge_small)
[![twitter](https://img.shields.io/twitter/follow/wasmcloud?style=social)](https://twitter.com/wasmcloud)
[![youtube subscribers](https://img.shields.io/youtube/channel/subscribers/UCmZVIWGxkudizD1Z1and5JA?style=social)](https://youtube.com/wasmcloud)
[![youtube views](https://img.shields.io/youtube/channel/views/UCmZVIWGxkudizD1Z1and5JA?style=social)](https://youtube.com/wasmcloud)

![wasmCloud logo](https://raw.githubusercontent.com/wasmCloud/branding/main/02.Horizontal%20Version/Pixel/PNG/Wasmcloud.Logo-Hrztl_Color.png)

wasmCloud is a **universal application platform** that helps you build and run globally distributed WebAssembly applications on any cloud or edge. Our goal is to make development more joyful and efficient by giving developers the tools to write only the code that matters—and making it easy to run that code anywhere.

wasmCloud leverages WebAssembly's security, portability, and performance to compose applications from tiny, independent building blocks.These building blocks are managed declaratively and reconfigurable at runtime. You shouldn't need to recompile your whole app to upgrade a database client or patch a vulnerability and you shouldn't need to recompile anything to move your app from development to production.

wasmCloud is designed around the following core tenets:

- Distributed from day one
- Run anywhere and everywhere
- Secure by default
- Faster iteration and lower maintenance

**Move from concept to production without changing your design, architecture, or your programming environment.**

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

wasmCloud is based on and implements [WASI 0.2 (aka the Component Model)][wasi-pt2]. WebAssembly Components that run on wasmCloud consume interfaces defined in [WIT][wit].

Want to get something running quickly? Check out the [`examples` directory of this repository](./examples). Examples are organized by programming language so you can easily find samples in your language of choice.

[wasi-pt2]: https://github.com/WebAssembly/WASI/blob/main/preview2/README.md
[wit]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md

### 💥 Awesome wasmCloud

For even more examples, check out [awesome projects](./awesome-wasmcloud) using wasmCloud from our community members!

# 🗺️ Roadmap and Vision

We have plenty of ideas and things going on in the wasmCloud project. Please check out the [Roadmap doc](https://wasmcloud.com/docs/roadmap) for more information, and the [wasmCloud Roadmap project](https://github.com/orgs/wasmCloud/projects/7/views/3) to see the status of new features.

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

wasmCloud uses some terminology you might not be familiar with. Check out the [concepts](https://wasmcloud.com/docs/concepts/interface-driven-development) section of our docs for a deeper dive.

---

## RPC Framework

wasmCloud uses an [RPC API](https://wasmcloud.com/docs/hosts/lattice-protocols/rpc) to enable seamless communication among the host runtime, components, and providers.

---

## Declarative Deployments

The **w**asmCloud **A**pplication **D**eployment **M**anager [wadm](https://github.com/wasmCloud/wadm) uses the Open Application Model to define and deploy application specifications.

---

## Host Runtimes

### 🦀 Rust Runtime

wasmCloud's [standard runtime](./crates/runtime) is built in Rust for its zero-cost abstractions, safety, security, and WebAssembly support.

### 🕸 JavaScript Runtime (`Experimental`)

For running a wasmCloud host in a browser or embedding in a JavaScript V8 host, use the [JavaScript Runtime](https://github.com/wasmCloud/wasmcloud-js)

---

## SDKs and libraries

### Rust Provider SDK

wasmCloud provides an [SDK](./crates/provider-sdk) for building capability providers in Rust.

### Provider Archive

[`provider-archive`](./crates/provider-archive) is a crate used to create Provider Archive (PAR) files. PARs are used to store, retrieve, and sign capability providers. Today, capability providers are distributed as binary files and run as system processes. In the future, wasmCloud aims to build capability providers as WebAssembly Components, which will remove the need for Provider Archives.

### `wasmcloud_actor` (`Experimental`)

[`wasmcloud_actor`](./crates/actor) is a wasmCloud actor library written in Rust which facilitates building of wasmCloud components.

The API of the crate matches closely what [`wit-bindgen`](https://github.com/bytecodealliance/wit-bindgen) would generate, meaning that one can switch from using plain `wit-bindgen`-generated bindings to `wasmcloud_actor` (and back) with minimal or no code changes.

### wascap

[`wascap`](./crates/wascap) is a low-level library used to insert and retrieve [claims](https://wasmcloud.com/docs/hosts/security#claims) on components and providers. Claims are part of wasmCloud's zero-trust security model.

---

_We are a Cloud Native Computing Foundation [sandbox project](https://www.cncf.io/sandbox-projects/)._

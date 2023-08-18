[![Homepage and Documentation](https://img.shields.io/website?label=Homepage&url=https%3A%2F%2Fwasmcloud.com)](https://wasmcloud.com)
[![CNCF sandbox project](https://img.shields.io/website?label=CNCF%20Sandbox%20Project&url=https://landscape.cncf.io/?selected=wasm-cloud)](https://landscape.cncf.io/?selected=wasm-cloud)
[![Stars](https://img.shields.io/github/stars/wasmcloud?color=gold&label=wasmCloud%20Org%20Stars)](https://github.com/wasmcloud/)
![Powered by WebAssembly](https://img.shields.io/badge/powered%20by-WebAssembly-orange.svg)<br />
[![reddit](https://img.shields.io/reddit/subreddit-subscribers/wasmcloud?style=social)](https://reddit.com/r/wasmcloud)
[![twitter](https://img.shields.io/twitter/follow/wasmcloud?style=social)](https://twitter.com/wasmcloud)
[![youtube subscribers](https://img.shields.io/youtube/channel/subscribers/UCmZVIWGxkudizD1Z1and5JA?style=social)](https://youtube.com/wasmcloud)
[![youtube views](https://img.shields.io/youtube/channel/views/UCmZVIWGxkudizD1Z1and5JA?style=social)](https://youtube.com/wasmcloud)

![wasmCloud logo](https://raw.githubusercontent.com/wasmCloud/branding/main/02.Horizontal%20Version/Pixel/PNG/Wasmcloud.Logo-Hrztl_Color.png)

# 💻 Distributed computing, _simplified_

The wasmCloud runtime is a vessel for running applications in the cloud, at the edge, in the browser, on small devices, and anywhere else you can imagine.

**We want to bring joy to distributed systems development without sacrificing enterprise-grade features.**

wasmCloud lets you focus on shipping _features_. Build secure, portable, re-usable components. Get rid of the headaches from being smothered by boilerplate, dependency hell, tight coupling, and designs mandated by your infrastructure.

## Core Tenets

- Productivity
- Portability
- Performance at any scale
- Enterprise-grade security
- Cost savings

# Getting Started

## Installation

Install the wasmCloud Shell (`wash`) with [one command](https://wasmcloud.com/docs/installation).

## Walkthrough

If you're new to the wasmCloud ecosystem, a great place to start is the [getting started](https://wasmcloud.com/docs/getting-started/) walkthrough.

## Examples

You can also take a look at a wide range of [examples](https://github.com/wasmCloud/examples/).

This includes actors, providers, interfaces, and full applications, including our [Petclinic microservices app](https://github.com/wasmCloud/examples/tree/main/petclinic) we've created to demonstrate how to design, compose, and build applications in wasmCloud.

### 💥 Awesome wasmCloud

For even more examples, check out [awesome projects](./awesome-wasmcloud) using wasmCloud from our community members!

# 🗺️ Roadmap and Vision

We have plenty of ideas and things going on in the wasmCloud project. Please check out the [Roadmap doc](https://wasmcloud.com/docs/roadmap) for more information, and the [wasmCloud Roadmap project](https://github.com/orgs/wasmCloud/projects/7/views/3) to see the status of new features.

# 🧑‍💻 Contributing

Want to get involved? For more information on how to contribute and our contributor guidelines, check out the [contributing readme](./CONTRIBUTING.md).

# 📚 Other Resources

## Reference Documentation

wasmCloud uses some terminology you might not be familiar with. Check out the [reference](https://wasmcloud.com/docs/category/reference) section of our docs for a deeper dive.

## RPC Framework

wasmCloud uses [wasmbus-rpc](https://github.com/wasmCloud/weld/tree/main/rpc-rs) to communicate between the host runtime, actors, and providers.

## Declarative Deployments

The **w**asmCloud **A**pplication **D**eployment **M**anager [wadm](https://github.com/wasmCloud/wadm) uses the Open Application Model to define and deploy application specifications.

## Host Runtimes

### ☁️ Elixir/OTP Runtime

The primary Cloud Native wasmCloud host runtime is the [Elixir/OTP](https://github.com/wasmCloud/wasmcloud-otp) runtime. wasmCloud leverages Elixir/OTP for its battle-tested, massively-scalable foundation; we leverage Rust for its zero-cost abstractions, safety, security, and WebAssembly support.

### 🦀 Rust Runtime (`Experimental`)

Rust runtime is under heavy development at the root of this repository.

### 🕸 JavaScript Runtime (`Experimental`)

For running a wasmCloud host in a browser or embedding in a JavaScript V8 host, use the [JavaScript Runtime](https://github.com/wasmCloud/wasmcloud-js)

## SDKs and libraries

### 🦀 `wasmcloud_runtime` (`Experimental`)

`wasmcloud_runtime` is a wasmCloud runtime library written in Rust and available at [`./crates/runtime`](./crates/runtime).

It is under heavy development, but is already used by Elixir/OTP and Rust host runtimes.

Bindings to other languages than Rust will be provided in the future.

Latest documentation is available at [wasmcloud.github.io/wasmCloud/wasmcloud_runtime](https://wasmcloud.github.io/wasmCloud/wasmcloud_runtime).

### 🦀 `wasmcloud_actor` (`Experimental`)

`wasmcloud_actor` is a wasmCloud actor library written in Rust and available at [`./crates/actor`](./crates/actor).

It provides functionality, which facilitates building of wasmCloud actors.

The API of the crate matches closely what [`wit-bindgen`](https://github.com/bytecodealliance/wit-bindgen) would generate, meaning that one can switch from using plain `wit-bindgen`-generated bindings to `wasmcloud_actor` (and back) with minimal or no code changes.

Latest documentation is available at [wasmcloud.github.io/wasmCloud/wasmcloud_actor](https://wasmcloud.github.io/wasmCloud/wasmcloud_actor/).

---

_We are a Cloud Native Computing Foundation [sandbox project](https://www.cncf.io/sandbox-projects/)._

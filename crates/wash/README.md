[![Latest Release](https://img.shields.io/github/v/release/wasmCloud/wasmCloud?filter=wash*)](https://github.com/wasmCloud/wasmCloud/releases)
[![Rust Build](https://img.shields.io/github/actions/workflow/status/wasmCloud/wasmCloud/wash.yml?branch=main)](https://github.com/wasmCloud/wasmCloud/actions/workflows/wash.yml)
[![Rust Version](https://img.shields.io/badge/rustc-1.66.0-orange.svg)](https://blog.rust-lang.org/2022/12/15/Rust-1.66.0.html)
[![Contributors](https://img.shields.io/github/contributors/wasmCloud/wasmCloud)](https://github.com/wasmCloud/wasmCloud/graphs/contributors)
[![Good first issues](https://img.shields.io/github/issues/wasmCloud/wasmCloud/good%20first%20issue?label=good%20first%20issues)](https://github.com/wasmCloud/wasmCloud/issues?q=is%3Aopen+is%3Aissue+label%3A%22good+first+issue%22+label%3A%22wash-cli%22)
[![wash-cli](https://img.shields.io/crates/v/wash-cli)](https://crates.io/crates/wash-cli)

```console
                                     _                 _    _____ _          _ _
                                ____| |               | |  / ____| |        | | |
 __      ____ _ ___ _ __ ___  / ____| | ___  _   _  __| | | (___ | |__   ___| | |
 \ \ /\ / / _` / __| '_ ` _ \| |    | |/ _ \| | | |/ _` |  \___ \| '_ \ / _ \ | |
  \ V  V / (_| \__ \ | | | | | |____| | (_) | |_| | (_| |  ____) | | | |  __/ | |
   \_/\_/ \__,_|___/_| |_| |_|\_____|_|\___/ \__,_|\__,_| |_____/|_| |_|\___|_|_|
```

> [!WARNING]
> This crate is being deprecated in favor of [wash](https://crates.io/crates/wash), where the wash CLI will be published from now on.

- [Why wash](#why-wash)
- [Installing wash](#installing-wash)
  - [Cargo](#cargo)
  - [Linux (deb/rpm + apt)](#linux-debrpm--apt)
  - [Linux (snap)](#linux-snap)
  - [Linux (brew)](#linux-brew)
  - [MacOS (brew)](#macos-brew)
  - [Windows (choco)](#windows-choco)
  - [Nix](#nix)
- [Proxy authentication](#proxy-authentication)
- [Using wash](#using-wash)
- [Shell auto-complete](#shell-auto-complete)
- [Contributing to wash](#contributing-to-wash)

## Why wash

`wash` is a bundle of command line tools that, together, form a comprehensive CLI for [wasmCloud](https://wasmcloud.com) development. Everything from generating new wasmCloud projects, starting local development infrastructure, interacting with OCI compliant registries, and deploying applications is contained within the subcommands of `wash`. Our goal with `wash` is to encapsulate our tools into a single binary to make developing WebAssembly with wasmCloud painless and simple.

## Installing wash

### Cargo

```bash
cargo install --locked wash
```

If you have [cargo-binstall](https://github.com/cargo-bins/cargo-binstall?tab=readme-ov-file#installation):

```bash
cargo binstall wash
```

### Linux (deb/rpm + apt)

```bash
# Debian / Ubuntu (deb)
curl -s https://packagecloud.io/install/repositories/wasmcloud/core/script.deb.sh | sudo bash
# Fedora (rpm)
curl -s https://packagecloud.io/install/repositories/wasmcloud/core/script.rpm.sh | sudo bash

sudo apt install wash
```

### Linux (snap)

```bash
sudo snap install wash --edge --devmode
```

### Linux (brew)

```bash
brew install wasmcloud/wasmcloud/wash
```

### MacOS (brew)

```bash
brew install wasmcloud/wasmcloud/wash
```

### Windows (choco)

```powershell
choco install wash
```

### Nix

```bash
nix run github:wasmCloud/wash
```

## Proxy authentication
In a scenario where you are behind a proxy, you can set the `HTTP_PROXY` and `HTTPS_PROXY` environment variables to the proxy URL.
And if your proxy requires authentication, you can set the `WASH_PROXY_USERNAME` and `WASH_PROXY_PASSWORD` environment variables to the username and password, respectively. Since most passwords contain special characters, it's recommended to specify the value for 'WASH_PROXY_PASSWORD' in single quotes.

For example, in a unix environment:

```console
export WASH_PROXY_USERNAME='username'
export WASH_PROXY_PASSWORD='p@ssw0rd'
```

## Using wash

`wash` has multiple subcommands, each specializing in one specific area of the wasmCloud development process.

```console
Build:
  new          Create a new project from a template
  build        Build (and sign) a wasmCloud component or capability provider
  dev          Start a developer loop to hot-reload a local wasmCloud component
  inspect      Inspect a capability provider or Wasm component for signing information and interfaces
  par          Create, inspect, and modify capability provider archive files

Run:
  up           Bootstrap a local wasmCloud environment
  down         Tear down a local wasmCloud environment (launched with wash up)
  app          Manage declarative applications and deployments (wadm)
  spy          Spy on all invocations a component sends and receives
  ui           Serve a web UI for wasmCloud

Iterate:
  get          Get information about different running wasmCloud resources
  start        Start a component or capability provider
  scale        Scale a component running in a host to a certain level of concurrency
  stop         Stop a component, capability provider, or host
  update       Update a component running in a host to newer image reference
  link         Link one component to another on a set of interfaces
  call         Invoke a simple function on a component running in a wasmCloud host
  label        Label (or un-label) a host with a key=value label pair
  config       Create configuration for components, capability providers and links

Publish:
  pull         Pull an artifact from an OCI compliant registry
  push         Push an artifact to an OCI compliant registry

Configure:
  completions  Generate shell completions for wash
  ctx          Manage wasmCloud host configuration contexts
  drain        Manage contents of local wasmCloud caches
  keys         Utilities for generating and managing signing keys
  claims       Generate and manage JWTs for wasmCloud components and capability providers
```

## Shell auto-complete

`wash` has support for autocomplete for Zsh, Bash, Fish, and PowerShell.
See [Completions](./Completions.md) for instructions for installing
autocomplete for your shell.

## Contributing to wash

Visit [CONTRIBUTING.md](./CONTRIBUTING.md) for more information on how to contribute to `wash` project.

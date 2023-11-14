[![Latest Release](https://img.shields.io/github/v/release/wasmCloud/wasmCloud?filter=wash*)](https://github.com/wasmCloud/wasmCloud/releases)
[![Rust Build](https://img.shields.io/github/actions/workflow/status/wasmCloud/wasmCloud/wash.yml?branch=main)](https://github.com/wasmCloud/wasmCloud/actions/workflows/wash.yml)
[![Rust Version](https://img.shields.io/badge/rustc-1.66.0-orange.svg)](https://blog.rust-lang.org/2022/12/15/Rust-1.66.0.html)
[![Contributors](https://img.shields.io/github/contributors/wasmCloud/wasmCloud)](https://github.com/wasmCloud/wasmCloud/graphs/contributors)
[![Good first issues](https://img.shields.io/github/issues/wasmCloud/wasmCloud/good%20first%20issue?label=good%20first%20issues)](https://github.com/wasmCloud/wasmCloud/issues?q=is%3Aopen+is%3Aissue+label%3A%22good+first+issue%22+label%3A%22wash-cli%22)
[![wash-cli](https://img.shields.io/crates/v/wash-cli)](https://crates.io/crates/wash-cli)

```
                                     _                 _    _____ _          _ _
                                ____| |               | |  / ____| |        | | |
 __      ____ _ ___ _ __ ___  / ____| | ___  _   _  __| | | (___ | |__   ___| | |
 \ \ /\ / / _` / __| '_ ` _ \| |    | |/ _ \| | | |/ _` |  \___ \| '_ \ / _ \ | |
  \ V  V / (_| \__ \ | | | | | |____| | (_) | |_| | (_| |  ____) | | | |  __/ | |
   \_/\_/ \__,_|___/_| |_| |_|\_____|_|\___/ \__,_|\__,_| |_____/|_| |_|\___|_|_|
```

- [Why wash](#why-wash)
- [Installing wash](#installing-wash)
  - [Cargo](#cargo)
  - [Linux (deb/rpm + apt)](#linux-debrpm--apt)
  - [Linux (snap)](#linux-snap)
  - [MacOS (brew)](#macos-brew)
  - [Windows (choco)](#windows-choco)
  - [Nix](#nix)
- [Using wash](#using-wash)
  - [build](#build)
  - [call](#call)
  - [claims](#claims)
  - [ctl](#ctl)
  - [ctx](#ctx)
  - [drain](#drain)
  - [gen](#gen)
  - [keys](#keys)
  - [lint](#lint)
  - [new](#new)
  - [par](#par)
  - [reg](#reg)
  - [up](#up)
  - [validate](#validate)
- [Contributing to wash](#contributing-to-wash)
  - [Developer guide](#developer-guide)

## Why wash

`wash` is a bundle of command line tools that, together, form a comprehensive CLI for [wasmCloud](https://wasmcloud.dev) development. Everything from generating new wasmCloud projects, managing cryptographic signing keys, and interacting with OCI compliant registries is contained within the subcommands of `wash`. Our goal with `wash` is to encapsulate our tools into a single binary to make developing WebAssembly with wasmCloud painless and simple.

## Installing wash

### Cargo

```
cargo install wash-cli
```

### Linux (deb/rpm + apt)

```
# Debian / Ubuntu (deb)
curl -s https://packagecloud.io/install/repositories/wasmcloud/core/script.deb.sh | sudo bash
# Fedora (rpm)
curl -s https://packagecloud.io/install/repositories/wasmcloud/core/script.rpm.sh | sudo bash

sudo apt install wash
```

### Linux (snap)

```
sudo snap install wash --edge --devmode
```

### Linux (brew)

```
brew install wash
```

### MacOS (brew)

```
brew install wash
```

### Windows (choco)

```powershell
choco install wash
```

### Nix

```
nix run github:wasmCloud/wash
```

## Using wash

`wash` has multiple subcommands, each specializing in one specific area of the wasmCloud development process.

### build

Builds and signs the actor, provider, or interface as defined in a `wasmcloud.toml` file.  Will look for configuration file in directory where command is being run.  
There are three main sections of a `wasmcloud.toml` file: common config, language config, and type config.

#### Common Config
| Setting       | Type   | Default                       | Description                                                                            |
| ------------- | ------ | ----------------------------- | -------------------------------------------------------------------------------------- |
| name          | string |                               | Name of the project                                                                    | 
| version       | string |                               | Semantic version of the project                                                        |
| path          | string | `{pwd}`                       | Path to the project directory to determine where built and signed artifacts are output | 
| wasm_bin_name | string | "name" setting                | Expected name of the wasm module binary that will be generated                         |
| language      | enum   | [rust, tinygo]                | Language that actor or provider is written in                                          |
| type          | enum   | [actor, provider, interface ] | Type of wasmcloud artifact that is being generated                                     |


#### Language Config - [tinygo]

> [!IMPORTANT]
> To build actors written in Go, `wash` uses the `tinygo` compiler toolchain. To set up TinyGo, we recommend the [official installation instructions](https://tinygo.org/getting-started/install/).

| Setting     | Type   | Default        | Description                   |
| ----------- | ------ | -------------- | ----------------------------- |
| tinygo_path | string | `which tinygo` | The path to the tinygo binary |

#### Language Config - [rust]

> [!IMPORTANT]
> To build actors written in Rust, `wash` uses the `cargo` toolchain. To set up Rust, we recommend using [`rustup`](https://doc.rust-lang.org/cargo/getting-started/installation.html#install-rust-and-cargo).

| Setting     | Type   | Default       | Description                             |
| ----------- | ------ | ------------- | --------------------------------------- |
| cargo_path  | string | `which cargo` | The path to the cargo binary            |
| target_path | string | ./target      | Path to cargo/rust's `target` directory |

#### Type Config - [actor]
| Setting | Type | Default | Description |
| ------- | ---- | ------- | ----------- |
| claims        | list    | []                     | The list of provider claims that this actor requires. eg. ["wasmcloud:httpserver", "wasmcloud:blobstore"] |
| registry      | string  | localhost:8080         | The registry to push to. eg. "localhost:8080"                                                                              |
| push_insecure | boolean | false | Whether to push to the registry insecurely                                                                                                  |
| key_directory | string  | `~/.wash/keys`         | The directory to store the private signing keys in                                                                        |
| filename      | string  | <build_output>_s.wasm  | The filename of the signed wasm actor                                                                                      |
| wasm_target   | string  | wasm32-unknown-unknown | Compile target                                                                                                            | 
| call_alias    | string  |                        |  The call alias of the actor |

#### Type Config - [provider]
| Setting       | Type   | Default | Description                       |
| ------------- | ------ | ------- | --------------------------------- |
| capability_id | string |         | The capability ID of the provider |
| vendor        | string |         | The vendor name of the provider   |

#### Type Config - [interface]
| Setting        | Type   | Default | Description               |
| -------------- | ------ | ------- | ------------------------- |
| html_target    | string | ./html  | Directory to output HTML  |
| codegen_config | string | .       | Path to codegen.toml file |

#### Example

```toml
name = "echo"
language = "rust"
type = "actor"
version = "0.1.0"

[actor]
claims = ["wasmcloud:httpserver"]

[rust]
cargo_path = "/tmp/cargo"
```

### call

Invoke a wasmCloud actor directly with a specified payload. This allows you to test actor handlers without the need to manage capabilities and link definitions for a rapid development feedback loop.

### claims

Generate JWTs for actors, capability providers, accounts and operators. Sign actor modules with claims including capability IDs, expiration, and keys to verify identity. Inspect actor modules to view their claims.

### completions

Generate shell completion files for Zsh, Bash, Fish, or PowerShell.

### ctl

Interact directly with a wasmCloud [control-interface](https://github.com/wasmCloud/control-interface), allowing you to imperatively schedule actors, providers and modify configurations of a wasmCloud host. Can be used to interact with local and remote control-interfaces.

### ctx

Automatically connect to your previously launched wasmCloud lattice with a managed context or use contexts to administer remote wasmCloud lattices.

### drain

Manage contents of the local wasmCloud cache. wasmCloud manages a local cache that will avoid redundant fetching of content when possible. `drain` allows you to manually clear that cache to ensure you're always pulling the latest versions of actors and providers that are hosted in remote OCI registries.

### gen

Generate code from [smithy](https://awslabs.github.io/smithy/index.html) files using [weld codegen](https://github.com/wasmCloud/weld/tree/main/codegen). This is the primary method of generating actor and capability provider code from .smithy interfaces. Currently has first class support for Rust actors and providers, along with autogenerated HTML documentation.

### keys

Generate ed25519 keys for securely signing and identifying wasmCloud entities (actors, providers, hosts). Read more about our decision to use ed25519 keys in our [ADR](https://wasmcloud.github.io/adr/0005-security-nkeys.html).

### lint

Perform lint checks on .smithy models, outputting warnings for best practices with interfaces.

### new

Create new wasmCloud projects from predefined [templates](https://github.com/wasmCloud/project-templates). This command is a one-stop-shop for creating new actors, providers, and interfaces for all aspects of your application.

### par

Create, modify and inspect [provider archives](https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-archive), a TAR format that contains a signed JWT and OS/Architecture specific binaries for native capability providers.

### reg

Push and Pull actors and capability providers to/from OCI compliant registries. Used extensively in our own CI/CD and in local development, where a local registry is used to store your development artifacts.

### up

Bootstrap a wasmCloud environment in one easy command, supporting both launching NATS and wasmCloud in the background as well as an "interactive" mode for shorter lived hosts.

### validate

Perform validation checks on .smithy models, ensuring that your interfaces are valid and usable for codegen and development.


## Shell auto-complete

`wash` has support for autocomplete for Zsh, Bash, Fish, and PowerShell.
See [Completions](./Completions.md) for instructions for installing
autocomplete for your shell.

## Contributing to wash

Visit [CONTRIBUTING.md](./CONTRIBUTING.md) for more information on how to contribute to `wash` project.

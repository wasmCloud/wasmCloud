# wasmCloud Releases

This document contains an overview of wasmCloud releases and how we manage and plan them. All other
subprojects (such as wasmcloud-operator, Wadm, and the Go SDKs) follow these same high level steps,
albeit with different released content.

## Table of Contents

- [wasmCloud Releases](#wasmcloud-releases)
  - [Table of Contents](#table-of-contents)
  - [Overview](#overview)
  - [Content](#content)
    - [Platform Support](#platform-support)
  - [Schedule](#schedule)
  - [Versioning](#versioning)
    - [Tag naming convention](#tag-naming-convention)

## Overview

The following is the high level overview of what goes into a wasmCloud release. 

- [Versioning](#versioning): All wasmCloud project use strict [semantic
  versioning](http://semver.org). Strict semver compatibility guarantees have always been and always
  will be part of the project.
- [Content](#content): The main wasmCloud/wasmCloud repo is a monorepo consisting of various
  projects, all versioned independently. Primary artifacts for any release are the binary artifacts
  built by automation
- [Platform Support](#platform-support): Because the project uses WebAssembly, cross-platform
  support is critical. The core wasmCloud host supports x86 and ARM64 targets and has releases for
  Linux, Windows, Mac, and Android (with more likely to be added). Core helper binaries and
  libraries like Wash and Wadm support Linux, Windows, and Mac.
- [Schedule](#schedule): Releases currently happen on an "as needed" basis, with future plans to
  move to scheduled releases as WebAssembly stabilizes.

## Content

The primary artifact of a release are the binary artifacts produced for various parts of the
monorepo. Below are the core binaries produced by our CI pipelines:

- wasmCloud host: binaries and a container images
- wash CLI tool: binaries and a container image
- Capability Providers: cross-platform binaries published as OCI Artifacts
- WIT Interface Definitions: Packaged as Wasm binaries and published as an OCI Artifact (using the
  [Wasm OCI guidance](https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/)
  published by TAG Runtime)

In addition to the core binaries, the project also publishes examples as Wasm binaries stored in
OCI.

All OCI artifacts and container images are published to `ghcr.io` under the `wasmcloud` namespace

Consumable Rust libraries (idiomatically called crates) are also published wherever possible to
enable reuse of common wasmCloud functionality in other Rust projects. These are published to
crates.io (with our [automation account](https://crates.io/users/automation-wasmcloud)) using the
[release runbook](./RELEASE_RUNBOOK.md) below.

### Platform Support

As a Wasm-centric project, wasmCloud must support most major operating systems and architectures. As
a rule, all projects should have binaries for and general support for at least the following
architectures and platforms:

- Linux x86
- Linux arm64
- Mac x86
- Mac arm64
- Windows x86

Wasm binaries by default support all of the above platforms and more.

## Schedule

Because the Wasm space is still evolving quickly, wasmCloud releases on an "as needed" basis or once
a month, whichever comes sooner. As a project, we do a quarterly planning session as a community
that takes place during our weekly community call. As maintainers and as a community, we discuss
upcoming features and likely target versions for those features. Generally, project maintainers will
communicate and decide on a release date, and sometimes requests for releasing a newer version
sooner (often tied to a newly released Wasm feature) come from the community.

As Wasm continues to stabilize, we will be switching to a release train style with scheduled release
dates (preferably monthly)

## Versioning

wasmCloud follows strict [semantic versioning](https://www.semver.org). Versions are declared with
git tags. Strict semver compatibility guarantees have always been and always will be part of the
project.

Backwards-incompatible changes are never added except in case of an issue with security.

### Tag naming convention

Within the wasmCloud monorepo, each project or binary has different tags. The top level tag (i.e.
`v1.3.1`) is reserved for the main wasmCloud host. All other tags follow the convention of
`$dash-separated-project-v$version` (e.g. `wash-v0.35.0`)

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Chore

 - <csr-id-b9770de23b8d3b0fa1adffddb94236403d7e1d3f/> bump `provider-sdk` to 0.2.0
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/> replace error field name with err
 - <csr-id-0023f7e86d5a40a534f623b7220743f27871549e/> reduce verbosity of instrumented functions
 - <csr-id-7b9ad7b57edd06c1c62833965041634811df47eb/> fix format

### New Features

 - <csr-id-bf396e0cea4dcb5baa0f0cb0201af0fb078f38a5/> update provider bindgen, add kvredis smithy-WIT implementation
 - <csr-id-813ce52a9c11270814eec051dfaa8817bf9f567d/> support chunking and dechunking of requests
 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end
 - <csr-id-c334d84d01b8b92ab9db105f8e6f0c4a6bcef8b1/> send OTEL config via HostData
 - <csr-id-ada90674df5130be6320788bcb08b7868f3b67a5/> add new provider SDK to repo
   This is now manually tested and in a state where I think we should have it
   in the repo. We should be able to keep iterating from there

### Bug Fixes

 - <csr-id-07d818cdbd50ae350d236fb1cc309d86b75739ea/> add what clippy took from me
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-c604aca1db1017e2458cf66eab232b081d615521/> enable `ansi` feature

### Other

 - <csr-id-4adbf0647f1ef987e92fbf927db9d09e64d3ecd8/> update `async-nats` to 0.33
 - <csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/> 'upstream/main' into `merge/wash`
 - <csr-id-d98a317b30e352ea0d73439ad3fa790ddfb8bf3f/> update opentelemetry

### Refactor

 - <csr-id-aea0a282911a704ee0d70ad38f267d8d8cc00d78/> convert blobstore-fs to bindgen
 - <csr-id-0319a9245589709d96b03786374d8026beb5d5d0/> move chunking to core
 - <csr-id-6f0a7d848e49d4cdc66dffe38fd8b41657f32649/> simply re-export wasmcloud_core as core
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers
 - <csr-id-3430c72b11564acc0624987cd3df08c629d7d197/> remove `atty` dependency

### Style

 - <csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/> replace needs_chunking function with direct comparison

### Refactor (BREAKING)

 - <csr-id-5fd0557c7ff454211e3f590333ff4dda208a1f7a/> make publish method crate-public
 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 27 commits contributed to the release over the course of 155 calendar days.
 - 25 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump `provider-sdk` to 0.2.0 (b9770de)
    - Make publish method crate-public (5fd0557)
    - Update `async-nats` to 0.33 (4adbf06)
    - Add descriptions to crates (cb0bcab)
    - 'upstream/main' into `merge/wash` (0f967b0)
    - Convert blobstore-fs to bindgen (aea0a28)
    - Replace error field name with err (3ffbd3a)
    - Update provider bindgen, add kvredis smithy-WIT implementation (bf396e0)
    - Reduce verbosity of instrumented functions (0023f7e)
    - Add cfg block to import (a810769)
    - Add what clippy took from me (07d818c)
    - Fix format (7b9ad7b)
    - Attach traces on inbound and outbound messages (74142c4)
    - Make content_length a required field (6428747)
    - Replace needs_chunking function with direct comparison (6de67aa)
    - Support chunking and dechunking of requests (813ce52)
    - Move chunking to core (0319a92)
    - Simply re-export wasmcloud_core as core (6f0a7d8)
    - Replace lazy_static with once_cell (e1d7356)
    - Construct a strongly typed HostData to send to providers (23f1759)
    - Support OTEL traces end-to-end (675d364)
    - Send OTEL config via HostData (c334d84)
    - Update opentelemetry (d98a317)
    - Enable `ansi` feature (c604aca)
    - Remove `atty` dependency (3430c72)
    - Merge pull request #396 from rvolosatovs/feat/provider-sdk (6ed04f0)
    - Add new provider SDK to repo (ada9067)
</details>


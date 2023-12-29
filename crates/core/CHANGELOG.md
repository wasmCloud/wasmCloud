# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Chore

 - <csr-id-eb0599fbdc6e1ac58616c7676b89bf7b19d4c662/> address clippy issues
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-8e071dde1a98caa7339e92882bb63c433ae2a042/> remove direct `wasmbus_rpc` dependency
 - <csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/> replace error field name with err

### New Features

 - <csr-id-813ce52a9c11270814eec051dfaa8817bf9f567d/> support chunking and dechunking of requests
 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end

### Bug Fixes

 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Refactor

 - <csr-id-123e53611e6d0b2bd4e92358783213784653fbf6/> convert httpclient provider to bindgen
   This commit converts the in-tree httpclient provider to use
   provider-wit-bindgen for it's implementation.
 - <csr-id-7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb/> clean-up imports
 - <csr-id-0319a9245589709d96b03786374d8026beb5d5d0/> move chunking to core

### Style

 - <csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/> replace needs_chunking function with direct comparison

### Chore (BREAKING)

 - <csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/> bump to 0.2.0 for async-nats release

### Refactor (BREAKING)

 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 16 commits contributed to the release over the course of 122 calendar days.
 - 15 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump to 0.2.0 for async-nats release (6abbcac)
    - Convert httpclient provider to bindgen (123e536)
    - Address clippy issues (eb0599f)
    - Clean-up imports (7402a1f)
    - Add descriptions to crates (cb0bcab)
    - Remove direct `wasmbus_rpc` dependency (8e071dd)
    - Replace error field name with err (3ffbd3a)
    - Allow namespaces with slashes (1829b27)
    - Include context on host errors (0e6e2da)
    - Look for invocation responses from providers (7502bcb)
    - Enable `std` anyhow feature (a896f05)
    - Make content_length a required field (6428747)
    - Replace needs_chunking function with direct comparison (6de67aa)
    - Support chunking and dechunking of requests (813ce52)
    - Move chunking to core (0319a92)
    - Support OTEL traces end-to-end (675d364)
</details>


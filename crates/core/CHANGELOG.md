# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-eb0599fbdc6e1ac58616c7676b89bf7b19d4c662/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-8e071dde1a98caa7339e92882bb63c433ae2a042/>
<csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/>
<csr-id-123e53611e6d0b2bd4e92358783213784653fbf6/>
<csr-id-7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb/>
<csr-id-0319a9245589709d96b03786374d8026beb5d5d0/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>
<csr-id-859b0baeff818a1af7e1824cbb80510669bdc976/>

### Chore

 - <csr-id-eb0599fbdc6e1ac58616c7676b89bf7b19d4c662/> address clippy issues
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-8e071dde1a98caa7339e92882bb63c433ae2a042/> remove direct `wasmbus_rpc` dependency
 - <csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/> replace error field name with err

### Chore

 - <csr-id-90d7c48a46e112ab884d9836bfc25c1de5570fee/> add changelogs for wash

### Chore

 - <csr-id-859b0baeff818a1af7e1824cbb80510669bdc976/> add changelogs for host

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

 - 18 commits contributed to the release over the course of 123 calendar days.
 - 17 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add changelogs for wash ([`90d7c48`](https://github.com/connorsmith256/wasmcloud/commit/90d7c48a46e112ab884d9836bfc25c1de5570fee))
    - Add changelogs for host ([`859b0ba`](https://github.com/connorsmith256/wasmcloud/commit/859b0baeff818a1af7e1824cbb80510669bdc976))
    - Bump to 0.2.0 for async-nats release ([`6abbcac`](https://github.com/connorsmith256/wasmcloud/commit/6abbcac954a9834d871ea69b8a40bd79d258c0f1))
    - Convert httpclient provider to bindgen ([`123e536`](https://github.com/connorsmith256/wasmcloud/commit/123e53611e6d0b2bd4e92358783213784653fbf6))
    - Address clippy issues ([`eb0599f`](https://github.com/connorsmith256/wasmcloud/commit/eb0599fbdc6e1ac58616c7676b89bf7b19d4c662))
    - Clean-up imports ([`7402a1f`](https://github.com/connorsmith256/wasmcloud/commit/7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb))
    - Add descriptions to crates ([`cb0bcab`](https://github.com/connorsmith256/wasmcloud/commit/cb0bcab822cb4290c673051ec1dd98d034a61546))
    - Remove direct `wasmbus_rpc` dependency ([`8e071dd`](https://github.com/connorsmith256/wasmcloud/commit/8e071dde1a98caa7339e92882bb63c433ae2a042))
    - Replace error field name with err ([`3ffbd3a`](https://github.com/connorsmith256/wasmcloud/commit/3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa))
    - Allow namespaces with slashes ([`1829b27`](https://github.com/connorsmith256/wasmcloud/commit/1829b27213e836cb347a542e9cdc771c74427892))
    - Include context on host errors ([`0e6e2da`](https://github.com/connorsmith256/wasmcloud/commit/0e6e2da7720e469b85940cadde3756b2afd64f7c))
    - Look for invocation responses from providers ([`7502bcb`](https://github.com/connorsmith256/wasmcloud/commit/7502bcb569420e2d402bf66d8a5eff2e6481a80b))
    - Enable `std` anyhow feature ([`a896f05`](https://github.com/connorsmith256/wasmcloud/commit/a896f05a35824f5e2ba16fdb1c1f5217c52a5388))
    - Make content_length a required field ([`6428747`](https://github.com/connorsmith256/wasmcloud/commit/642874717b6aab760d4692f9e8b12803548314e2))
    - Replace needs_chunking function with direct comparison ([`6de67aa`](https://github.com/connorsmith256/wasmcloud/commit/6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06))
    - Support chunking and dechunking of requests ([`813ce52`](https://github.com/connorsmith256/wasmcloud/commit/813ce52a9c11270814eec051dfaa8817bf9f567d))
    - Move chunking to core ([`0319a92`](https://github.com/connorsmith256/wasmcloud/commit/0319a9245589709d96b03786374d8026beb5d5d0))
    - Support OTEL traces end-to-end ([`675d364`](https://github.com/connorsmith256/wasmcloud/commit/675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6))
</details>


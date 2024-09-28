# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (2024-09-28)

### Chore

 - <csr-id-81ab5914e7d08740eb9371c9b718f13f0419c23f/> enable `ring` feature for `async-nats`
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### New Features

 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/> enable OTEL logs

### Other

 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 7 commits contributed to the release.
 - 323 days passed between releases.
 - 7 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump for test-util release ([`7cd2e71`](https://github.com/wasmCloud/wasmCloud/commit/7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4))
    - Enable `ring` feature for `async-nats` ([`81ab591`](https://github.com/wasmCloud/wasmCloud/commit/81ab5914e7d08740eb9371c9b718f13f0419c23f))
    - Generate changelogs after 1.0.1 release ([`4e0313a`](https://github.com/wasmCloud/wasmCloud/commit/4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e))
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Enable OTEL logs ([`3602bdf`](https://github.com/wasmCloud/wasmCloud/commit/3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3))
</details>

## 0.1.1 (2024-07-31)

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-81ab5914e7d08740eb9371c9b718f13f0419c23f/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### Chore

 - <csr-id-81ab5914e7d08740eb9371c9b718f13f0419c23f/> enable `ring` feature for `async-nats`
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features

### New Features

 - <csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/> enable OTEL logs
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

## v0.1.0 (2023-11-09)

<csr-id-b0e3cedcc2bb8ee5c4f852e5ee44e07ce95dd7a2/>
<csr-id-22276ff61bcb4992b557f7af6624c9715f72c32b/>

### Chore

 - <csr-id-b0e3cedcc2bb8ee5c4f852e5ee44e07ce95dd7a2/> document opentelemetry-nats

### Bug Fixes

 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses

### Other

 - <csr-id-22276ff61bcb4992b557f7af6624c9715f72c32b/> update dependencies

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Document opentelemetry-nats ([`b0e3ced`](https://github.com/wasmCloud/wasmCloud/commit/b0e3cedcc2bb8ee5c4f852e5ee44e07ce95dd7a2))
    - Update dependencies ([`22276ff`](https://github.com/wasmCloud/wasmCloud/commit/22276ff61bcb4992b557f7af6624c9715f72c32b))
    - Attach traces on inbound and outbound messages ([`74142c4`](https://github.com/wasmCloud/wasmCloud/commit/74142c4cff683565fb321b7b65fbb158b5a9c990))
</details>


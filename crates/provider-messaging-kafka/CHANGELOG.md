# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-955a6893792e86292883e76de57434616c28d380/> update `messaging` to `0.2.0`
 - <csr-id-4da9d22ea1c578a80107ed010ac174baa46f6a05/> remove contract_id
   While we have yet to figure out exactly how we expose WIT related
   metadata about the provider to the
   host (see: https://github.com/wasmCloud/wasmCloud/issues/1780), we
   won't be needing the wasmcloud contract specific code that was
   necessary before.
   
   This commit removes `contract_id()` as a requirement for providers.

### Documentation

 - <csr-id-abb09690ff0fc5d835abd93ed98e045404b5e96b/> remove --capid usage in README

### New Features

 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`

### Bug Fixes

 - <csr-id-5d645087bc73a3a000fa4184ea768527ca90acda/> add OTEL for messaging kafka provider
   This commit ensures OTEL is working for the messaging-kafka provider.

### Refactor

 - <csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/> return wrapped `WrpcClient` directly

### Bug Fixes (BREAKING)

 - <csr-id-903955009340190283c813fa225bae514fb15c03/> rename actor to component

### Refactor (BREAKING)

 - <csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/> make providers part of the workspace

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 10 commits contributed to the release over the course of 36 calendar days.
 - 10 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add OTEL for messaging kafka provider ([`5d64508`](https://github.com/wasmCloud/wasmCloud/commit/5d645087bc73a3a000fa4184ea768527ca90acda))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Rename actor to component ([`9039550`](https://github.com/wasmCloud/wasmCloud/commit/903955009340190283c813fa225bae514fb15c03))
    - Update `wrpc:keyvalue` in providers ([`9cd2b40`](https://github.com/wasmCloud/wasmCloud/commit/9cd2b4034f8d5688ce250429dc14120eaf61b483))
    - Return wrapped `WrpcClient` directly ([`87eb6c8`](https://github.com/wasmCloud/wasmCloud/commit/87eb6c8b2c0bd31def1cfdc6121c612c4dc90871))
    - Switch to `wit-bindgen-wrpc` ([`322f471`](https://github.com/wasmCloud/wasmCloud/commit/322f471f9a8154224a50ec33517c9f5b1716d2d5))
    - Update `messaging` to `0.2.0` ([`955a689`](https://github.com/wasmCloud/wasmCloud/commit/955a6893792e86292883e76de57434616c28d380))
    - Remove contract_id ([`4da9d22`](https://github.com/wasmCloud/wasmCloud/commit/4da9d22ea1c578a80107ed010ac174baa46f6a05))
    - Remove --capid usage in README ([`abb0969`](https://github.com/wasmCloud/wasmCloud/commit/abb09690ff0fc5d835abd93ed98e045404b5e96b))
    - Make providers part of the workspace ([`005b707`](https://github.com/wasmCloud/wasmCloud/commit/005b7073e6896f68aa64348fef44ae69305acaf7))
</details>


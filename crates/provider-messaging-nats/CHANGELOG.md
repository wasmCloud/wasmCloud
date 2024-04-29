# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-955a6893792e86292883e76de57434616c28d380/>
<csr-id-4da9d22ea1c578a80107ed010ac174baa46f6a05/>
<csr-id-073b3c21581632f135d47b14b6b13ad13d7d7592/>
<csr-id-f032a962c6f1c5e1988fb65fd62ad4bc89dd1e54/>
<csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/>
<csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/>
<csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/>

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

### Chore

 - <csr-id-5b1ba2bd545038381c03c5b68615982f360caaf6/> bump to 0.20.0

### New Features

 - <csr-id-71df023d931085ee4ab3e7ff43dade9da161e19b/> add context if missing to messaging-nats-provider
 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-f56492ac6b5e6f1274a1f11b061c42cace372122/> migrate to `wrpc:keyvalue`
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs

### Bug Fixes

 - <csr-id-27cb86d9e86b09c2da9e23a4ebfbddf22f3abad2/> wasmcloud messaging provider directionality

### Other

 - <csr-id-073b3c21581632f135d47b14b6b13ad13d7d7592/> sync with `capability-providers`
 - <csr-id-f032a962c6f1c5e1988fb65fd62ad4bc89dd1e54/> bump provider versions
   bump to next minor version after the version reported at
   https://github.com/wasmCloud/capability-providers

### Refactor

 - <csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/> return wrapped `WrpcClient` directly
 - <csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/> remove `ProviderHandler`

### Bug Fixes (BREAKING)

 - <csr-id-903955009340190283c813fa225bae514fb15c03/> rename actor to component

### Refactor (BREAKING)

 - <csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/> make providers part of the workspace

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 16 commits contributed to the release over the course of 41 calendar days.
 - 16 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Generate crate changelogs ([`cda9f72`](https://github.com/wasmCloud/wasmCloud/commit/cda9f724d2d2e4ea55006a43b166d18875148c48))
    - Bump to 0.20.0 ([`5b1ba2b`](https://github.com/wasmCloud/wasmCloud/commit/5b1ba2bd545038381c03c5b68615982f360caaf6))
    - Wasmcloud messaging provider directionality ([`27cb86d`](https://github.com/wasmCloud/wasmCloud/commit/27cb86d9e86b09c2da9e23a4ebfbddf22f3abad2))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Add context if missing to messaging-nats-provider ([`71df023`](https://github.com/wasmCloud/wasmCloud/commit/71df023d931085ee4ab3e7ff43dade9da161e19b))
    - Rename actor to component ([`9039550`](https://github.com/wasmCloud/wasmCloud/commit/903955009340190283c813fa225bae514fb15c03))
    - Update `wrpc:keyvalue` in providers ([`9cd2b40`](https://github.com/wasmCloud/wasmCloud/commit/9cd2b4034f8d5688ce250429dc14120eaf61b483))
    - Return wrapped `WrpcClient` directly ([`87eb6c8`](https://github.com/wasmCloud/wasmCloud/commit/87eb6c8b2c0bd31def1cfdc6121c612c4dc90871))
    - Switch to `wit-bindgen-wrpc` ([`322f471`](https://github.com/wasmCloud/wasmCloud/commit/322f471f9a8154224a50ec33517c9f5b1716d2d5))
    - Remove `ProviderHandler` ([`8082135`](https://github.com/wasmCloud/wasmCloud/commit/8082135282f66b5d56fe6d14bb5ce6dc510d4b63))
    - Update `messaging` to `0.2.0` ([`955a689`](https://github.com/wasmCloud/wasmCloud/commit/955a6893792e86292883e76de57434616c28d380))
    - Remove contract_id ([`4da9d22`](https://github.com/wasmCloud/wasmCloud/commit/4da9d22ea1c578a80107ed010ac174baa46f6a05))
    - Migrate to `wrpc:keyvalue` ([`f56492a`](https://github.com/wasmCloud/wasmCloud/commit/f56492ac6b5e6f1274a1f11b061c42cace372122))
    - Sync with `capability-providers` ([`073b3c2`](https://github.com/wasmCloud/wasmCloud/commit/073b3c21581632f135d47b14b6b13ad13d7d7592))
    - Bump provider versions ([`f032a96`](https://github.com/wasmCloud/wasmCloud/commit/f032a962c6f1c5e1988fb65fd62ad4bc89dd1e54))
    - Make providers part of the workspace ([`005b707`](https://github.com/wasmCloud/wasmCloud/commit/005b7073e6896f68aa64348fef44ae69305acaf7))
</details>


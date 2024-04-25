# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Chore

 - <csr-id-4da9d22ea1c578a80107ed010ac174baa46f6a05/> remove contract_id
   While we have yet to figure out exactly how we expose WIT related
   metadata about the provider to the
   host (see: https://github.com/wasmCloud/wasmCloud/issues/1780), we
   won't be needing the wasmcloud contract specific code that was
   necessary before.
   
   This commit removes `contract_id()` as a requirement for providers.

### Refactor

 - <csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/> return wrapped `WrpcClient` directly
 - <csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/> remove `ProviderHandler`

### Refactor (BREAKING)

 - <csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/> make providers part of the workspace

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release over the course of 36 calendar days.
 - 4 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Return wrapped `WrpcClient` directly ([`87eb6c8`](https://github.com/wasmCloud/wasmCloud/commit/87eb6c8b2c0bd31def1cfdc6121c612c4dc90871))
    - Remove `ProviderHandler` ([`8082135`](https://github.com/wasmCloud/wasmCloud/commit/8082135282f66b5d56fe6d14bb5ce6dc510d4b63))
    - Remove contract_id ([`4da9d22`](https://github.com/wasmCloud/wasmCloud/commit/4da9d22ea1c578a80107ed010ac174baa46f6a05))
    - Make providers part of the workspace ([`005b707`](https://github.com/wasmCloud/wasmCloud/commit/005b7073e6896f68aa64348fef44ae69305acaf7))
</details>


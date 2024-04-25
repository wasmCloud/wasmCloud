# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### New Features

 - <csr-id-74353eeeb1ee7c1023c296c92b21369e48a1a66b/> pass along tracing context for kv-redis
 - <csr-id-e48d562740be942349d3834b56a75a7cab0b560c/> pass along tracing context for kv-redis
 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk
 - <csr-id-f56492ac6b5e6f1274a1f11b061c42cace372122/> migrate to `wrpc:keyvalue`

### Other

 - <csr-id-073b3c21581632f135d47b14b6b13ad13d7d7592/> sync with `capability-providers`
 - <csr-id-f032a962c6f1c5e1988fb65fd62ad4bc89dd1e54/> bump provider versions
   bump to next minor version after the version reported at
   https://github.com/wasmCloud/capability-providers

### Refactor

 - <csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/> remove `ProviderHandler`

### New Features (BREAKING)

 - <csr-id-91874e9f4bf2b37b895a4654250203144e12815c/> convert to `wrpc:blobstore`

### Bug Fixes (BREAKING)

 - <csr-id-903955009340190283c813fa225bae514fb15c03/> rename actor to component

### Refactor (BREAKING)

 - <csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/> make providers part of the workspace

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 13 commits contributed to the release over the course of 36 calendar days.
 - 13 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Pass along tracing context for kv-redis ([`74353ee`](https://github.com/wasmCloud/wasmCloud/commit/74353eeeb1ee7c1023c296c92b21369e48a1a66b))
    - Pass along tracing context for kv-redis ([`e48d562`](https://github.com/wasmCloud/wasmCloud/commit/e48d562740be942349d3834b56a75a7cab0b560c))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Rename actor to component ([`9039550`](https://github.com/wasmCloud/wasmCloud/commit/903955009340190283c813fa225bae514fb15c03))
    - Update `wrpc:keyvalue` in providers ([`9cd2b40`](https://github.com/wasmCloud/wasmCloud/commit/9cd2b4034f8d5688ce250429dc14120eaf61b483))
    - Switch to `wit-bindgen-wrpc` ([`322f471`](https://github.com/wasmCloud/wasmCloud/commit/322f471f9a8154224a50ec33517c9f5b1716d2d5))
    - Remove `ProviderHandler` ([`8082135`](https://github.com/wasmCloud/wasmCloud/commit/8082135282f66b5d56fe6d14bb5ce6dc510d4b63))
    - Introduce provider interface sdk ([`a84492d`](https://github.com/wasmCloud/wasmCloud/commit/a84492d15d154a272de33680f6338379fc036a3a))
    - Migrate to `wrpc:keyvalue` ([`f56492a`](https://github.com/wasmCloud/wasmCloud/commit/f56492ac6b5e6f1274a1f11b061c42cace372122))
    - Convert to `wrpc:blobstore` ([`91874e9`](https://github.com/wasmCloud/wasmCloud/commit/91874e9f4bf2b37b895a4654250203144e12815c))
    - Sync with `capability-providers` ([`073b3c2`](https://github.com/wasmCloud/wasmCloud/commit/073b3c21581632f135d47b14b6b13ad13d7d7592))
    - Bump provider versions ([`f032a96`](https://github.com/wasmCloud/wasmCloud/commit/f032a962c6f1c5e1988fb65fd62ad4bc89dd1e54))
    - Make providers part of the workspace ([`005b707`](https://github.com/wasmCloud/wasmCloud/commit/005b7073e6896f68aa64348fef44ae69305acaf7))
</details>


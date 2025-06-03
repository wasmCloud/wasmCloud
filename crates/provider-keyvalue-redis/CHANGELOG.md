# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-073b3c21581632f135d47b14b6b13ad13d7d7592/>
<csr-id-f032a962c6f1c5e1988fb65fd62ad4bc89dd1e54/>
<csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/>
<csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### Bug Fixes

 - <csr-id-63fb1ebbe7c89e962d170753d1224826641c31d4/> ignore URL config key vase for kv redis provider

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features

### New Features

 - <csr-id-74353eeeb1ee7c1023c296c92b21369e48a1a66b/> pass along tracing context for kv-redis
 - <csr-id-e48d562740be942349d3834b56a75a7cab0b560c/> pass along tracing context for kv-redis
 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk
 - <csr-id-f56492ac6b5e6f1274a1f11b061c42cace372122/> migrate to `wrpc:keyvalue`
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-8b9d96b7391938d95519200e54dd3d68159cd67e/> allow missing default connection for redis
   Without a default connection the redis KV provider would normally
   fail to start -- this can be quite confusing considering usually a
   connetion is not expected to be made yet.
   
   This commit refactors the conenction logic to allow running a provider
   without a pre-made default connection, and creates one upon the first
   connection of an actor that relies on the default
   connection (i.e. doesn't have a connection specified via link config).
 - <csr-id-4ef1a370cb94b0dc7f07cbde051e8f8239f32adc/> implement wasi:kevalue/batch
   Implement the wasi:keyvalue/batch interface for the Redis keyvalue
   provider.
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

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

 - 18 commits contributed to the release over the course of 41 calendar days.
 - 18 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - Ignore URL config key vase for kv redis provider ([`63fb1eb`](https://github.com/wasmCloud/wasmCloud/commit/63fb1ebbe7c89e962d170753d1224826641c31d4))
    - Allow missing default connection for redis ([`8b9d96b`](https://github.com/wasmCloud/wasmCloud/commit/8b9d96b7391938d95519200e54dd3d68159cd67e))
    - Implement wasi:kevalue/batch ([`4ef1a37`](https://github.com/wasmCloud/wasmCloud/commit/4ef1a370cb94b0dc7f07cbde051e8f8239f32adc))
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


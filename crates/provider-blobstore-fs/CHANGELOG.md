# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.6.0 (2024-04-25)

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### New Features

 - <csr-id-6d4ad85067c5d6c59895b2721dbb363747c130bd/> pass along OTEL context for blobstore-fs
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk

### Bug Fixes

 - <csr-id-c32159a870d3b36a412be2e2904f44f4c42e1e2a/> Fixes missing import
   Not quite sure how the original PR passed tests, but we were missing an
   import. This should fix the issue
 - <csr-id-92051dfd897f26e91a3cdd71bcb3cc58ef55fab8/> support windows

### Other

 - <csr-id-f032a962c6f1c5e1988fb65fd62ad4bc89dd1e54/> bump provider versions
   bump to next minor version after the version reported at
   https://github.com/wasmCloud/capability-providers

### Refactor

 - <csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/> remove `ProviderHandler`
 - <csr-id-8ef4d158b7c263a1741da06d66e30ed787b22144/> don't seek if start is 0
 - <csr-id-4ce65d0e76d6d918a586fc984c87ab50cf5fa695/> simplify `blobstore-fs` reading

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
    - Pass along OTEL context for blobstore-fs ([`6d4ad85`](https://github.com/wasmCloud/wasmCloud/commit/6d4ad85067c5d6c59895b2721dbb363747c130bd))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Rename actor to component ([`9039550`](https://github.com/wasmCloud/wasmCloud/commit/903955009340190283c813fa225bae514fb15c03))
    - Switch to `wit-bindgen-wrpc` ([`322f471`](https://github.com/wasmCloud/wasmCloud/commit/322f471f9a8154224a50ec33517c9f5b1716d2d5))
    - Remove `ProviderHandler` ([`8082135`](https://github.com/wasmCloud/wasmCloud/commit/8082135282f66b5d56fe6d14bb5ce6dc510d4b63))
    - Introduce provider interface sdk ([`a84492d`](https://github.com/wasmCloud/wasmCloud/commit/a84492d15d154a272de33680f6338379fc036a3a))
    - Fixes missing import ([`c32159a`](https://github.com/wasmCloud/wasmCloud/commit/c32159a870d3b36a412be2e2904f44f4c42e1e2a))
    - Don't seek if start is 0 ([`8ef4d15`](https://github.com/wasmCloud/wasmCloud/commit/8ef4d158b7c263a1741da06d66e30ed787b22144))
    - Simplify `blobstore-fs` reading ([`4ce65d0`](https://github.com/wasmCloud/wasmCloud/commit/4ce65d0e76d6d918a586fc984c87ab50cf5fa695))
    - Convert to `wrpc:blobstore` ([`91874e9`](https://github.com/wasmCloud/wasmCloud/commit/91874e9f4bf2b37b895a4654250203144e12815c))
    - Bump provider versions ([`f032a96`](https://github.com/wasmCloud/wasmCloud/commit/f032a962c6f1c5e1988fb65fd62ad4bc89dd1e54))
    - Support windows ([`92051df`](https://github.com/wasmCloud/wasmCloud/commit/92051dfd897f26e91a3cdd71bcb3cc58ef55fab8))
    - Make providers part of the workspace ([`005b707`](https://github.com/wasmCloud/wasmCloud/commit/005b7073e6896f68aa64348fef44ae69305acaf7))
</details>


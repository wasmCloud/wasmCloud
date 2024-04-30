# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-f032a962c6f1c5e1988fb65fd62ad4bc89dd1e54/>
<csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/>
<csr-id-8ef4d158b7c263a1741da06d66e30ed787b22144/>
<csr-id-4ce65d0e76d6d918a586fc984c87ab50cf5fa695/>
<csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### Refactor

 - <csr-id-d88617fa32e3e6bbfc5e9eb9874cad677fdbd886/> more informative file open error in blobstore

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-e5c48eaac36788372ce6e3d52b266c5514bc3e37/> bump to v0.6.1

### New Features

 - <csr-id-6d4ad85067c5d6c59895b2721dbb363747c130bd/> pass along OTEL context for blobstore-fs
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

### Bug Fixes

 - <csr-id-c32159a870d3b36a412be2e2904f44f4c42e1e2a/> Fixes missing import
   Not quite sure how the original PR passed tests, but we were missing an
   import. This should fix the issue
 - <csr-id-92051dfd897f26e91a3cdd71bcb3cc58ef55fab8/> support windows
 - <csr-id-1958903640c068ab61508e4dfd2ae47d23e09e5b/> support creation time fallback

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

 - 18 commits contributed to the release over the course of 41 calendar days.
 - 18 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - More informative file open error in blobstore ([`d88617f`](https://github.com/wasmCloud/wasmCloud/commit/d88617fa32e3e6bbfc5e9eb9874cad677fdbd886))
    - Bump to v0.6.1 ([`e5c48ea`](https://github.com/wasmCloud/wasmCloud/commit/e5c48eaac36788372ce6e3d52b266c5514bc3e37))
    - Support creation time fallback ([`1958903`](https://github.com/wasmCloud/wasmCloud/commit/1958903640c068ab61508e4dfd2ae47d23e09e5b))
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


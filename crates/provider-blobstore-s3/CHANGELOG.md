# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-2badff2d8b7f791f8930272c3556bd9cf41c665b/>
<csr-id-e8f8d8732ca1ed01993aadc27a37bf66892633bc/>
<csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/>
<csr-id-00b98e1f15f61c500f57f0f4cb3ccb29834d99a9/>
<csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-2badff2d8b7f791f8930272c3556bd9cf41c665b/> remove redundant `tower_service` dep

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features

### New Features

 - <csr-id-1e8fd3cacdd9eb097f3ec1f554858fabff76f5b9/> pass along OTEL context for blobstore-s3
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk
 - <csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/> use native TLS roots along webpki
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

### Other

 - <csr-id-e8f8d8732ca1ed01993aadc27a37bf66892633bc/> Bump fastrand from 1.9.0 to 2.0.1
   Bumps [fastrand](https://github.com/smol-rs/fastrand) from 1.9.0 to 2.0.1.
   - [Release notes](https://github.com/smol-rs/fastrand/releases)
   - [Changelog](https://github.com/smol-rs/fastrand/blob/master/CHANGELOG.md)
   - [Commits](https://github.com/smol-rs/fastrand/compare/v1.9.0...v2.0.1)
   
   ---
   updated-dependencies:
   - dependency-name: fastrand
     dependency-type: direct:production
     update-type: version-update:semver-major
   ...

### Refactor

 - <csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/> remove `ProviderHandler`
 - <csr-id-00b98e1f15f61c500f57f0f4cb3ccb29834d99a9/> clean-up configuration

### New Features (BREAKING)

 - <csr-id-91874e9f4bf2b37b895a4654250203144e12815c/> convert to `wrpc:blobstore`

### Bug Fixes (BREAKING)

 - <csr-id-903955009340190283c813fa225bae514fb15c03/> rename actor to component

### Refactor (BREAKING)

 - <csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/> make providers part of the workspace

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 14 commits contributed to the release over the course of 41 calendar days.
 - 14 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - Pass along OTEL context for blobstore-s3 ([`1e8fd3c`](https://github.com/wasmCloud/wasmCloud/commit/1e8fd3cacdd9eb097f3ec1f554858fabff76f5b9))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Rename actor to component ([`9039550`](https://github.com/wasmCloud/wasmCloud/commit/903955009340190283c813fa225bae514fb15c03))
    - Switch to `wit-bindgen-wrpc` ([`322f471`](https://github.com/wasmCloud/wasmCloud/commit/322f471f9a8154224a50ec33517c9f5b1716d2d5))
    - Remove `ProviderHandler` ([`8082135`](https://github.com/wasmCloud/wasmCloud/commit/8082135282f66b5d56fe6d14bb5ce6dc510d4b63))
    - Introduce provider interface sdk ([`a84492d`](https://github.com/wasmCloud/wasmCloud/commit/a84492d15d154a272de33680f6338379fc036a3a))
    - Use native TLS roots along webpki ([`07b5e70`](https://github.com/wasmCloud/wasmCloud/commit/07b5e70a7f1321d184962d7197a8d98d1ecaaf71))
    - Clean-up configuration ([`00b98e1`](https://github.com/wasmCloud/wasmCloud/commit/00b98e1f15f61c500f57f0f4cb3ccb29834d99a9))
    - Convert to `wrpc:blobstore` ([`91874e9`](https://github.com/wasmCloud/wasmCloud/commit/91874e9f4bf2b37b895a4654250203144e12815c))
    - Bump fastrand from 1.9.0 to 2.0.1 ([`e8f8d87`](https://github.com/wasmCloud/wasmCloud/commit/e8f8d8732ca1ed01993aadc27a37bf66892633bc))
    - Remove redundant `tower_service` dep ([`2badff2`](https://github.com/wasmCloud/wasmCloud/commit/2badff2d8b7f791f8930272c3556bd9cf41c665b))
    - Make providers part of the workspace ([`005b707`](https://github.com/wasmCloud/wasmCloud/commit/005b7073e6896f68aa64348fef44ae69305acaf7))
</details>


# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.3.0 (2024-07-31)

### Chore

 - <csr-id-a65fa0a21f00ae82c2aef7377946fa96904e5dfb/> update secrets-types and client to 0.2.0
 - <csr-id-353874525cf033dd83758c657a626b768c7ee8e6/> Add description for wasmcloud-secrets-client crate
 - <csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/> update nats-kv version to v1alpha1

### New Features

 - <csr-id-55eae4ed6fe7454f10b6290d8f993ac802c855a5/> Add wasmcloud-secrets-client

### Bug Fixes

 - <csr-id-b2e7df8fb0729b1c33d4453ad0279a981c45b25d/> supply features

### Other

 - <csr-id-f985ae56a4df91fc135d89bca5d4626a586c586d/> setup secrets-client for release

### Refactor

 - <csr-id-669f36aae36ee8b50704cdc943ed4ffa1d325e94/> add usage of thiserror
   This commit introduces `thiserror` for the errors in the
   `secrets-client` crate.
 - <csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/> improve error usage of bail

### New Features (BREAKING)

 - <csr-id-797f96a51f7153bed12a0c3ecef221d0c91cd934/> support field on SecretRequest

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 9 commits contributed to the release over the course of 13 calendar days.
 - 9 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update secrets-types and client to 0.2.0 ([`a65fa0a`](https://github.com/wasmCloud/wasmCloud/commit/a65fa0a21f00ae82c2aef7377946fa96904e5dfb))
    - Support field on SecretRequest ([`797f96a`](https://github.com/wasmCloud/wasmCloud/commit/797f96a51f7153bed12a0c3ecef221d0c91cd934))
    - Setup secrets-client for release ([`f985ae5`](https://github.com/wasmCloud/wasmCloud/commit/f985ae56a4df91fc135d89bca5d4626a586c586d))
    - Add description for wasmcloud-secrets-client crate ([`3538745`](https://github.com/wasmCloud/wasmCloud/commit/353874525cf033dd83758c657a626b768c7ee8e6))
    - Update nats-kv version to v1alpha1 ([`13edb3e`](https://github.com/wasmCloud/wasmCloud/commit/13edb3e395eeb304adb88fcda0ebf1ada2c295c4))
    - Add usage of thiserror ([`669f36a`](https://github.com/wasmCloud/wasmCloud/commit/669f36aae36ee8b50704cdc943ed4ffa1d325e94))
    - Supply features ([`b2e7df8`](https://github.com/wasmCloud/wasmCloud/commit/b2e7df8fb0729b1c33d4453ad0279a981c45b25d))
    - Improve error usage of bail ([`c30bf33`](https://github.com/wasmCloud/wasmCloud/commit/c30bf33f754c15122ead7f041b7d3e063dd1db33))
    - Add wasmcloud-secrets-client ([`55eae4e`](https://github.com/wasmCloud/wasmCloud/commit/55eae4ed6fe7454f10b6290d8f993ac802c855a5))
</details>


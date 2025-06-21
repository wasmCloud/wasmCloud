# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.8.0 (2025-05-28)

### Chore

 - <csr-id-3078c88f0ebed96027e20997bccc1c125583fad4/> bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0
 - <csr-id-a65fa0a21f00ae82c2aef7377946fa96904e5dfb/> update secrets-types and client to 0.2.0
 - <csr-id-353874525cf033dd83758c657a626b768c7ee8e6/> Add description for wasmcloud-secrets-client crate
 - <csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/> update nats-kv version to v1alpha1

### New Features

 - <csr-id-55eae4ed6fe7454f10b6290d8f993ac802c855a5/> Add wasmcloud-secrets-client

### Bug Fixes

 - <csr-id-b2e7df8fb0729b1c33d4453ad0279a981c45b25d/> supply features

### Other

 - <csr-id-ef45f597710929d41be989110fc3c51621c9ee62/> bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates
   SAFETY BUMP: wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wash-cli v0.37.0, wasmcloud-host v0.23.0, wasmcloud-runtime v0.7.0, wasmcloud-test-util v0.15.0, wasmcloud-secrets-client v0.6.0
 - <csr-id-da9659b0ec70127ba8bcf0bf5c0d018d3e8da140/> wasmcloud-secrets-client v0.5.0
 - <csr-id-8e8efa3cbb765918c7c88f71a74c520a49965efb/> wasmcloud-secrets-client v0.4.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-835b49613f7f0d6903ad53d78f49c17db1e3d90e/> release and update CHANGELOG
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
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

 - 16 commits contributed to the release over the course of 314 calendar days.
 - 16 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0 ([`3078c88`](https://github.com/wasmCloud/wasmCloud/commit/3078c88f0ebed96027e20997bccc1c125583fad4))
    - Bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates ([`ef45f59`](https://github.com/wasmCloud/wasmCloud/commit/ef45f597710929d41be989110fc3c51621c9ee62))
    - Wasmcloud-secrets-client v0.5.0 ([`da9659b`](https://github.com/wasmCloud/wasmCloud/commit/da9659b0ec70127ba8bcf0bf5c0d018d3e8da140))
    - Wasmcloud-secrets-client v0.4.0 ([`8e8efa3`](https://github.com/wasmCloud/wasmCloud/commit/8e8efa3cbb765918c7c88f71a74c520a49965efb))
    - Bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates ([`8403350`](https://github.com/wasmCloud/wasmCloud/commit/8403350432a2387d4a2bce9c096f002005ba54be))
    - Release and update CHANGELOG ([`835b496`](https://github.com/wasmCloud/wasmCloud/commit/835b49613f7f0d6903ad53d78f49c17db1e3d90e))
    - Bump for test-util release ([`7cd2e71`](https://github.com/wasmCloud/wasmCloud/commit/7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4))
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

## v0.4.0 (2024-09-06)

<csr-id-a65fa0a21f00ae82c2aef7377946fa96904e5dfb/>
<csr-id-353874525cf033dd83758c657a626b768c7ee8e6/>
<csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-835b49613f7f0d6903ad53d78f49c17db1e3d90e/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-f985ae56a4df91fc135d89bca5d4626a586c586d/>
<csr-id-669f36aae36ee8b50704cdc943ed4ffa1d325e94/>
<csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/>

### Chore

 - <csr-id-a65fa0a21f00ae82c2aef7377946fa96904e5dfb/> update secrets-types and client to 0.2.0
 - <csr-id-353874525cf033dd83758c657a626b768c7ee8e6/> Add description for wasmcloud-secrets-client crate
 - <csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/> update nats-kv version to v1alpha1

### New Features

 - <csr-id-55eae4ed6fe7454f10b6290d8f993ac802c855a5/> Add wasmcloud-secrets-client

### Bug Fixes

 - <csr-id-b2e7df8fb0729b1c33d4453ad0279a981c45b25d/> supply features

### Other

 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-835b49613f7f0d6903ad53d78f49c17db1e3d90e/> release and update CHANGELOG
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-f985ae56a4df91fc135d89bca5d4626a586c586d/> setup secrets-client for release

### Refactor

 - <csr-id-669f36aae36ee8b50704cdc943ed4ffa1d325e94/> add usage of thiserror
   This commit introduces `thiserror` for the errors in the
   `secrets-client` crate.
 - <csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/> improve error usage of bail

### New Features (BREAKING)

 - <csr-id-797f96a51f7153bed12a0c3ecef221d0c91cd934/> support field on SecretRequest

## v0.3.0 (2024-07-31)

<csr-id-a65fa0a21f00ae82c2aef7377946fa96904e5dfb/>
<csr-id-353874525cf033dd83758c657a626b768c7ee8e6/>
<csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/>
<csr-id-f985ae56a4df91fc135d89bca5d4626a586c586d/>
<csr-id-669f36aae36ee8b50704cdc943ed4ffa1d325e94/>
<csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/>

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


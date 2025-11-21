# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.9.0 (2025-11-21)

### Chore

 - <csr-id-69d94666354c32e63c276847dcfdf9c80dab9b66/> bumps for wasmcloud-test-util release
   SAFETY BUMP: wasmcloud-host v0.26.0, wasmcloud-test-util v0.18.0
 - <csr-id-3078c88f0ebed96027e20997bccc1c125583fad4/> bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0
 - <csr-id-a65fa0a21f00ae82c2aef7377946fa96904e5dfb/> update secrets-types and client to 0.2.0
 - <csr-id-353874525cf033dd83758c657a626b768c7ee8e6/> Add description for wasmcloud-secrets-client crate
 - <csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/> update nats-kv version to v1alpha1

### New Features

 - <csr-id-55eae4ed6fe7454f10b6290d8f993ac802c855a5/> Add wasmcloud-secrets-client

### Bug Fixes

 - <csr-id-b2e7df8fb0729b1c33d4453ad0279a981c45b25d/> supply features

### Other

 - <csr-id-fd2b59f9ebb24cf3068a5f5897ba16d0689a4e84/> update dependencies
   • Updated input 'nixify':
       'github:rvolosatovs/nixify/a4f73e104d2652be5014b22578a78562d664cbe4?narHash=sha256-G0MDm0p46SKBY0HA1L4kLD6zjvubJ20BaAMtgeCOa%2BQ%3D' (2025-06-19)
     → 'github:rvolosatovs/nixify/ff8c6fb0b714a67cc926f588d7361cda4256de1d?narHash=sha256-gk68gRVLxA1sCNiKKpJpUkE6Xqn0hWYccAJ6ExHl2QA%3D' (2025-07-16)
   • Updated input 'nixify/advisory-db':
       'github:rustsec/advisory-db/7573f55ba337263f61167dbb0ea926cdc7c8eb5d?narHash=sha256-il%2BCAqChFIB82xP6bO43dWlUVs%2BNlG7a4g8liIP5HcI%3D' (2025-06-17)
     → 'github:rustsec/advisory-db/c67f7726a9188b40c37534589293fec688892e42?narHash=sha256-89kooFbF4ij1QSHAiyfD694U3BnsRsTI8xefsDxjMBU%3D' (2025-07-14)
   • Updated input 'nixify/crane':
       'github:ipetkov/crane/e37c943371b73ed87faf33f7583860f81f1d5a48?narHash=sha256-tL42YoNg9y30u7zAqtoGDNdTyXTi8EALDeCB13FtbQA%3D' (2025-06-18)
     → 'github:ipetkov/crane/471f8cd756349f4e86784ea10fdc9ccb91711fca?narHash=sha256-T1XWEFfw%2BiNrvlRczZS4BkaZJ5W3Z2Xp%2B31P2IShJj8%3D' (2025-07-16)
   • Updated input 'nixify/fenix':
       'github:nix-community/fenix/770345287ea0845c38d15bd750226a96250a30f0?narHash=sha256-M5y3WuvyFwr6Xw3d2xnmBCpTaz/87GR8mib%2BnLLDGIQ%3D' (2025-06-18)
     → 'github:nix-community/fenix/d17ca03c15660ecb8e5a01ca34e441f594feec62?narHash=sha256-pPqES/udciKmKo422mfwRQ3YzjUCVyCTOsgZYA1xh%2Bg%3D' (2025-07-15)
   • Updated input 'nixify/fenix/rust-analyzer-src':
       'github:rust-lang/rust-analyzer/5d93e31067f2344e1401ffe5323796122403e10e?narHash=sha256-N4Sfk43%2BlsOcjWQE8SsuML0WovWRT53vPbO8PebAJXg%3D' (2025-06-17)
     → 'github:rust-lang/rust-analyzer/e10d64eb402a25a32d9f1ef60cacc89d82a01b85?narHash=sha256-V2nHrCJ0/Pv30j8NWJ4GfDlaNzfkOdYI0jS69GdVpq8%3D' (2025-07-14)
   • Updated input 'nixify/nixpkgs-darwin':
       'github:nixos/nixpkgs/8f49bca3dc47f48ef46511613450364fd82b0b36?narHash=sha256-v2Ai/K9AS0aEw6%2BPCu4WrU1f9I98hWdxG9EnjRw5uXM%3D' (2025-06-17)
     → 'github:nixos/nixpkgs/1156bb3c3d94de7c6a7dc798b42c98bb975f3a75?narHash=sha256-RaaMPRtewLITsV0JMIgoTSkSR%2BWuu/a/I/Za0hiCes8%3D' (2025-07-14)
   • Updated input 'nixify/nixpkgs-nixos':
       'github:nixos/nixpkgs/36ab78dab7da2e4e27911007033713bab534187b?narHash=sha256-urV51uWH7fVnhIvsZIELIYalMYsyr2FCalvlRTzqWRw%3D' (2025-06-17)
     → 'github:nixos/nixpkgs/dfcd5b901dbab46c9c6e80b265648481aafb01f8?narHash=sha256-Kt1UIPi7kZqkSc5HVj6UY5YLHHEzPBkgpNUByuyxtlw%3D' (2025-07-13)
   • Updated input 'nixify/rust-overlay':
       'github:oxalica/rust-overlay/f9b2b2b1327ff6beab4662b8ea41689e0a57b8d4?narHash=sha256-1kniuhH70q4TAC/xIvjFYH46aHiLrbIlcr6fdrRwO1A%3D' (2025-06-18)
     → 'github:oxalica/rust-overlay/9127ca1f5a785b23a2fc1c74551a27d3e8b9a28b?narHash=sha256-0vUE42ji4mcCvQO8CI0Oy8LmC6u2G4qpYldZbZ26MLc%3D' (2025-07-15)
   • Updated input 'nixlib':
       'github:nix-community/nixpkgs.lib/14a40a1d7fb9afa4739275ac642ed7301a9ba1ab?narHash=sha256-urW/Ylk9FIfvXfliA1ywh75yszAbiTEVgpPeinFyVZo%3D' (2025-06-29)
     → 'github:nix-community/nixpkgs.lib/9100109c11b6b5482ea949c980b86e24740dca08?narHash=sha256-jj/HBJFSapTk4LfeJgNLk2wEE2BO6dgBYVRbXMNOCeM%3D' (2025-07-20)
   • Updated input 'nixpkgs-unstable':
       'github:NixOS/nixpkgs/b95255df2360a45ddbb03817a68869d5cb01bf96?narHash=sha256-IJWIzZSkBsDzS7iS/iwSwur%2BxFkWqeLYC4kdf8ObtOM%3D' (2025-06-30)
     → 'github:NixOS/nixpkgs/83e677f31c84212343f4cc553bab85c2efcad60a?narHash=sha256-XSQy6wRKHhRe//iVY5lS/ZpI/Jn6crWI8fQzl647wCg%3D' (2025-07-22)
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

 - 18 commits contributed to the release over the course of 491 calendar days.
 - 18 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update dependencies ([`fd2b59f`](https://github.com/wasmCloud/wasmCloud/commit/fd2b59f9ebb24cf3068a5f5897ba16d0689a4e84))
    - Bumps for wasmcloud-test-util release ([`69d9466`](https://github.com/wasmCloud/wasmCloud/commit/69d94666354c32e63c276847dcfdf9c80dab9b66))
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

## v0.8.0 (2025-05-28)

<csr-id-3078c88f0ebed96027e20997bccc1c125583fad4/>
<csr-id-a65fa0a21f00ae82c2aef7377946fa96904e5dfb/>
<csr-id-353874525cf033dd83758c657a626b768c7ee8e6/>
<csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/>
<csr-id-ef45f597710929d41be989110fc3c51621c9ee62/>
<csr-id-da9659b0ec70127ba8bcf0bf5c0d018d3e8da140/>
<csr-id-8e8efa3cbb765918c7c88f71a74c520a49965efb/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-835b49613f7f0d6903ad53d78f49c17db1e3d90e/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-f985ae56a4df91fc135d89bca5d4626a586c586d/>
<csr-id-669f36aae36ee8b50704cdc943ed4ffa1d325e94/>
<csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/>

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


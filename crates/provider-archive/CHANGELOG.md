# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.16.2 (2026-01-29)

### Chore

 - <csr-id-146d1ecf4396d21e19f595cedb87c29696171880/> wasmCloud-host v0.27.0
   Bump provider-archive v0.16.1, wasmcloud-core v0.21.0, wasmcloud-tracing v0.16.0, wasmcloud-provider-sdk v0.17.0, wasmcloud-runtime v0.12.0, wasmcloud-secrets-types v0.8.0, wasmcloud-secrets-client v0.9.0, wasmcloud-host v0.27.0, safety bump 7 crates
   
   SAFETY BUMP: wasmcloud-tracing v0.16.0, wasmcloud-provider-sdk v0.17.0, wasmcloud-runtime v0.12.0, wasmcloud-secrets-client v0.9.0, wasmcloud-host v0.27.0, wasmcloud-test-util v0.19.0, secrets-nats-kv v0.3.0

### Bug Fixes

 - <csr-id-8a3891f6c3145c0f7321bec17c51cbf043f3b16b/> remove component spec feature
   This commit removes the component spec feature that was introduced in
   2025, leaving the rest of the other changes that occurred in the
   meantime intact.
 - <csr-id-a0c85491995af8852f91d6ebf1856f84f16199b6/> Fix CVE-2025-62518 by migrating to astral-tokio-tar

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release over the course of 93 calendar days.
 - 315 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Remove component spec feature ([`8a3891f`](https://github.com/wasmCloud/wasmCloud/commit/8a3891f6c3145c0f7321bec17c51cbf043f3b16b))
    - WasmCloud-host v0.27.0 ([`146d1ec`](https://github.com/wasmCloud/wasmCloud/commit/146d1ecf4396d21e19f595cedb87c29696171880))
    - Fix CVE-2025-62518 by migrating to astral-tokio-tar ([`a0c8549`](https://github.com/wasmCloud/wasmCloud/commit/a0c85491995af8852f91d6ebf1856f84f16199b6))
</details>

## 0.16.1 (2025-11-21)

### Bug Fixes

 - <csr-id-a0c85491995af8852f91d6ebf1856f84f16199b6/> Fix CVE-2025-62518 by migrating to astral-tokio-tar

## 0.16.0 (2025-03-19)

<csr-id-3078c88f0ebed96027e20997bccc1c125583fad4/>

### Chore

 - <csr-id-3078c88f0ebed96027e20997bccc1c125583fad4/> bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 41 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0 ([`3078c88`](https://github.com/wasmCloud/wasmCloud/commit/3078c88f0ebed96027e20997bccc1c125583fad4))
</details>

## 0.15.0 (2025-02-06)

<csr-id-583957aed6918ad08b6d1c9b937e0811b05ee0a7/>
<csr-id-98bf9ae36fc321fc7803a7485f32b833907f036e/>
<csr-id-a9b02e5e203c1332ee785cbee8b998be9584c5b0/>
<csr-id-ef45f597710929d41be989110fc3c51621c9ee62/>
<csr-id-81766005150677e6d21c28eee27f2240b3da39fc/>
<csr-id-c50884ac5e7ca8c58bb8a14ef499b0e4ee7d0530/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-63cb8c96b2a047a7e884aecef82bdffd5319f545/>

### Chore

 - <csr-id-583957aed6918ad08b6d1c9b937e0811b05ee0a7/> Update provider-archive version
 - <csr-id-98bf9ae36fc321fc7803a7485f32b833907f036e/> address clippy warnings
 - <csr-id-a9b02e5e203c1332ee785cbee8b998be9584c5b0/> address clippy warnings

### New Features

 - <csr-id-dccf7fb6b635d82096e55d8400ea5e9f48ff546b/> add WIT interface embedding in provider archives

### Other

 - <csr-id-ef45f597710929d41be989110fc3c51621c9ee62/> bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates
   SAFETY BUMP: wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wash-cli v0.37.0, wasmcloud-host v0.23.0, wasmcloud-runtime v0.7.0, wasmcloud-test-util v0.15.0, wasmcloud-secrets-client v0.6.0
 - <csr-id-81766005150677e6d21c28eee27f2240b3da39fc/> update to 0.13
 - <csr-id-c50884ac5e7ca8c58bb8a14ef499b0e4ee7d0530/> update deps for wash-cli-v0.36
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0

### Refactor

 - <csr-id-63cb8c96b2a047a7e884aecef82bdffd5319f545/> use wit_component::encode instead of dummy module

### New Features (BREAKING)

 - <csr-id-6df433f01db7923d11554e2b76b9cdd34452e6f5/> store claims and jwt with par

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 10 commits contributed to the release over the course of 208 calendar days.
 - 237 days passed between releases.
 - 10 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update provider-archive version ([`583957a`](https://github.com/wasmCloud/wasmCloud/commit/583957aed6918ad08b6d1c9b937e0811b05ee0a7))
    - Use wit_component::encode instead of dummy module ([`63cb8c9`](https://github.com/wasmCloud/wasmCloud/commit/63cb8c96b2a047a7e884aecef82bdffd5319f545))
    - Add WIT interface embedding in provider archives ([`dccf7fb`](https://github.com/wasmCloud/wasmCloud/commit/dccf7fb6b635d82096e55d8400ea5e9f48ff546b))
    - Address clippy warnings ([`98bf9ae`](https://github.com/wasmCloud/wasmCloud/commit/98bf9ae36fc321fc7803a7485f32b833907f036e))
    - Bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates ([`ef45f59`](https://github.com/wasmCloud/wasmCloud/commit/ef45f597710929d41be989110fc3c51621c9ee62))
    - Update to 0.13 ([`8176600`](https://github.com/wasmCloud/wasmCloud/commit/81766005150677e6d21c28eee27f2240b3da39fc))
    - Update deps for wash-cli-v0.36 ([`c50884a`](https://github.com/wasmCloud/wasmCloud/commit/c50884ac5e7ca8c58bb8a14ef499b0e4ee7d0530))
    - Bump for test-util release ([`7cd2e71`](https://github.com/wasmCloud/wasmCloud/commit/7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4))
    - Store claims and jwt with par ([`6df433f`](https://github.com/wasmCloud/wasmCloud/commit/6df433f01db7923d11554e2b76b9cdd34452e6f5))
    - Address clippy warnings ([`a9b02e5`](https://github.com/wasmCloud/wasmCloud/commit/a9b02e5e203c1332ee785cbee8b998be9584c5b0))
</details>

## 0.14.0 (2024-11-08)

<csr-id-a9b02e5e203c1332ee785cbee8b998be9584c5b0/>
<csr-id-4213d3c378c49c930723313060195653157d63ef/>
<csr-id-c50884ac5e7ca8c58bb8a14ef499b0e4ee7d0530/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>

### Chore

 - <csr-id-a9b02e5e203c1332ee785cbee8b998be9584c5b0/> address clippy warnings

### Other

 - <csr-id-4213d3c378c49c930723313060195653157d63ef/> update to 0.13
 - <csr-id-c50884ac5e7ca8c58bb8a14ef499b0e4ee7d0530/> update deps for wash-cli-v0.36
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0

### New Features (BREAKING)

 - <csr-id-6df433f01db7923d11554e2b76b9cdd34452e6f5/> store claims and jwt with par

## 0.12.0 (2024-07-31)

<csr-id-a9b02e5e203c1332ee785cbee8b998be9584c5b0/>

### Chore

 - <csr-id-a9b02e5e203c1332ee785cbee8b998be9584c5b0/> address clippy warnings

### New Features (BREAKING)

 - <csr-id-6df433f01db7923d11554e2b76b9cdd34452e6f5/> store claims and jwt with par

## 0.11.0 (2024-06-13)

<csr-id-c86f24b61e68d48104e89e0aecf2a914aebaad6b/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-569f5636c924c855c1098f63cd9521e2f2e65fa2/>

### Chore

 - <csr-id-c86f24b61e68d48104e89e0aecf2a914aebaad6b/> bump to v0.11.0 for release
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### New Features

 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

### Refactor

 - <csr-id-569f5636c924c855c1098f63cd9521e2f2e65fa2/> more informative file open error

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 8 commits contributed to the release over the course of 56 calendar days.
 - 56 days passed between releases.
 - 6 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump to v0.11.0 for release ([`c86f24b`](https://github.com/wasmCloud/wasmCloud/commit/c86f24b61e68d48104e89e0aecf2a914aebaad6b))
    - Bump provider-archive v0.10.2, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0 ([`73c0ef0`](https://github.com/wasmCloud/wasmCloud/commit/73c0ef0bbe2f6b525655939d2cd30740aef4b6bc))
    - Bump provider-archive v0.10.1, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0, safety bump 5 crates ([`75a2e52`](https://github.com/wasmCloud/wasmCloud/commit/75a2e52f52690ba143679c90237851ebd07e153f))
    - Generate changelogs after 1.0.1 release ([`4e0313a`](https://github.com/wasmCloud/wasmCloud/commit/4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e))
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - More informative file open error ([`569f563`](https://github.com/wasmCloud/wasmCloud/commit/569f5636c924c855c1098f63cd9521e2f2e65fa2))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
</details>

## 0.10.2 (2024-05-08)

<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-569f5636c924c855c1098f63cd9521e2f2e65fa2/>

### Chore

 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### New Features

 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

### Refactor

 - <csr-id-569f5636c924c855c1098f63cd9521e2f2e65fa2/> more informative file open error

## 0.10.1 (2024-05-08)

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-569f5636c924c855c1098f63cd9521e2f2e65fa2/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### Chore

 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release

### Refactor

 - <csr-id-569f5636c924c855c1098f63cd9521e2f2e65fa2/> more informative file open error

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features

### New Features

 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

## v0.10.0 (2024-04-17)

<csr-id-de379871b3741d50223229c1b0b1fc118f9dd028/>

### Documentation

 - <csr-id-9e48b5d1c6952b254f973b672633cb934fecfa49/> remove `capid` from docs

### Other

 - <csr-id-de379871b3741d50223229c1b0b1fc118f9dd028/> v0.10.0

### New Features (BREAKING)

 - <csr-id-3c56e8f18e7e40982c59ee911140cd5965c733f5/> remove capabilities

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release over the course of 16 calendar days.
 - 30 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - V0.10.0 ([`de37987`](https://github.com/wasmCloud/wasmCloud/commit/de379871b3741d50223229c1b0b1fc118f9dd028))
    - Remove `capid` from docs ([`9e48b5d`](https://github.com/wasmCloud/wasmCloud/commit/9e48b5d1c6952b254f973b672633cb934fecfa49))
    - Remove capabilities ([`3c56e8f`](https://github.com/wasmCloud/wasmCloud/commit/3c56e8f18e7e40982c59ee911140cd5965c733f5))
</details>

## v0.9.0 (2024-03-17)

<csr-id-6b52afa7b8af453234574fe7e5116c512521f4be/>
<csr-id-18791e7666b4de2526628e2a973c47b7f51d9481/>
<csr-id-ee9d552c7ea1c017d8aa646f64002a85ffebefb8/>
<csr-id-9de9ae3de8799661525b2458303e72cd24cd666f/>

### Chore

 - <csr-id-6b52afa7b8af453234574fe7e5116c512521f4be/> bump to 0.9
 - <csr-id-18791e7666b4de2526628e2a973c47b7f51d9481/> integrate `control-interface` into the workspace
 - <csr-id-ee9d552c7ea1c017d8aa646f64002a85ffebefb8/> address `clippy` warnings in workspace
 - <csr-id-9de9ae3de8799661525b2458303e72cd24cd666f/> integrate `provider-archive` into the workspace

### Documentation

 - <csr-id-05ac449d3da207fd495ecbd786220b053fd6300e/> actor to components terminology
   This change only updates documentation terminology
   to use components instead of actors.
   
   Examples will use the terminology components as well so
   I'm opting to rename the example directories now ahead
   of any source code changes for actor to component
   renames.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 11 commits contributed to the release over the course of 1048 calendar days.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 3 unique issues were worked on: [#191](https://github.com/wasmCloud/wasmCloud/issues/191), [#241](https://github.com/wasmCloud/wasmCloud/issues/241), [#249](https://github.com/wasmCloud/wasmCloud/issues/249)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#191](https://github.com/wasmCloud/wasmCloud/issues/191)**
    - Add provider-archive to the crates/ directory ([`5cc74ce`](https://github.com/wasmCloud/wasmCloud/commit/5cc74ce950184de2c9cc3a4ea9b344d1fe98ed00))
 * **[#241](https://github.com/wasmCloud/wasmCloud/issues/241)**
    - Relocation for deprecation ([`915534b`](https://github.com/wasmCloud/wasmCloud/commit/915534b8cf4266c0b6ba3738765f5f68196d8943))
 * **[#249](https://github.com/wasmCloud/wasmCloud/issues/249)**
    - Add pinned resources for the pre-otp host ([`28840af`](https://github.com/wasmCloud/wasmCloud/commit/28840af8b417752430797acb5d2b1bb6c977f717))
 * **Uncategorized**
    - Bump to 0.9 ([`6b52afa`](https://github.com/wasmCloud/wasmCloud/commit/6b52afa7b8af453234574fe7e5116c512521f4be))
    - Actor to components terminology ([`05ac449`](https://github.com/wasmCloud/wasmCloud/commit/05ac449d3da207fd495ecbd786220b053fd6300e))
    - Merge pull request #927 from rvolosatovs/merge/control-interface ([`5d40fcb`](https://github.com/wasmCloud/wasmCloud/commit/5d40fcb06f4a029cca05f0d5b5f8c12722553822))
    - Integrate `control-interface` into the workspace ([`18791e7`](https://github.com/wasmCloud/wasmCloud/commit/18791e7666b4de2526628e2a973c47b7f51d9481))
    - Merge pull request #762 from rvolosatovs/merge/wascap ([`89570cc`](https://github.com/wasmCloud/wasmCloud/commit/89570cc8d7ac7fbf6acd83fdf91f2ac8014d0b77))
    - Address `clippy` warnings in workspace ([`ee9d552`](https://github.com/wasmCloud/wasmCloud/commit/ee9d552c7ea1c017d8aa646f64002a85ffebefb8))
    - Integrate `provider-archive` into the workspace ([`9de9ae3`](https://github.com/wasmCloud/wasmCloud/commit/9de9ae3de8799661525b2458303e72cd24cd666f))
    - Add 'crates/provider-archive/' from commit '5a5eb500efff41baacb664dd569f0f70c77a7451' ([`79638b9`](https://github.com/wasmCloud/wasmCloud/commit/79638b96654cdf1426531424fd82043d663db725))
</details>


# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### Refactor

 - <csr-id-569f5636c924c855c1098f63cd9521e2f2e65fa2/> more informative file open error

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features

### New Features

 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release over the course of 12 calendar days.
 - 12 days passed between releases.
 - 4 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - More informative file open error ([`569f563`](https://github.com/wasmCloud/wasmCloud/commit/569f5636c924c855c1098f63cd9521e2f2e65fa2))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
</details>

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


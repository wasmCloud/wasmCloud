# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.16.2 (2026-01-29)

### Chore

 - <csr-id-1d8076df4694c707ae691c8a286e65eef4ab1f37/> bump version

### Bug Fixes

 - <csr-id-5345c9b30156dc1bdf721d3634d6bcaf15561238/> add default endpoints for OpenTelemetry collectors
   Add sensible default endpoints when no explicit OTEL endpoints are provided:
   - HTTP protocol: http://127.0.0.1:4318/v1/<signal> (traces/metrics/logs)
   - gRPC protocol: http://127.0.0.1:4317
   
   This resolves the issue where wasmCloud would fail to configure OTEL
   exporters with "invalid URI empty string" errors when no endpoints
   were explicitly configured.
   
   Also includes:
   - Updated CLI documentation to reflect default endpoints
   - Fixed clippy warning using std::io::Error::other()
   - Updated tests to expect new default behavior

### Test

 - <csr-id-4ddb109b4f0966c305d84faa690b6a14b57ded08/> ignore invalid fixutre tests

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release over the course of 246 calendar days.
 - 315 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Ignore invalid fixutre tests ([`4ddb109`](https://github.com/wasmCloud/wasmCloud/commit/4ddb109b4f0966c305d84faa690b6a14b57ded08))
    - Bump version ([`1d8076d`](https://github.com/wasmCloud/wasmCloud/commit/1d8076df4694c707ae691c8a286e65eef4ab1f37))
    - Add default endpoints for OpenTelemetry collectors ([`5345c9b`](https://github.com/wasmCloud/wasmCloud/commit/5345c9b30156dc1bdf721d3634d6bcaf15561238))
</details>

## v0.15.3 (2025-01-28)

### Chore

 - <csr-id-8b16ec5c8dbfa7b776a7360524c7e11c3fa16a32/> bump to 0.15.3 for release
 - <csr-id-6659528a4531f8d8024785296a36874b7e409f31/> fix spelling
 - <csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/> bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, wasmcloud-host v0.22.0, wasmcloud-runtime v0.6.0, wasmcloud-test-util v0.14.0
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests

### New Features

 - <csr-id-0b164fde352a782a1b3c8a451e5b5bb791505556/> add Host entity type
   Add a `Host` entity type to wascap. This allows us to generate JWTs for
   hosts that contains assertions about the metadata they were started
   with. For now this only includes host labels, but this could change in
   the future.
 - <csr-id-add7bb1e11bf76eb235f7aa7b7c6ef7db93bae5e/> add Host entity type
   Add a `Host` entity type to wascap. This allows us to generate JWTs for
   hosts that contains assertions about the metadata they were started
   with. For now this only includes host labels, but this could change in
   the future.

### Bug Fixes

 - <csr-id-77576260a0a3de3771b77c0f2f2a050c2a300ef7/> update to nuid 0.5 api
 - <csr-id-eb9621bddd9febe38b70fae4372ddd74f9031295/> enable new component model feature

### Other

 - <csr-id-ef45f597710929d41be989110fc3c51621c9ee62/> bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates
   SAFETY BUMP: wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wash-cli v0.37.0, wasmcloud-host v0.23.0, wasmcloud-runtime v0.7.0, wasmcloud-test-util v0.15.0, wasmcloud-secrets-client v0.6.0
 - <csr-id-f128cec29f07ae84e37822c5bba1c91eeb9d82fd/> release and update CHANGELOG

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 11 commits contributed to the release over the course of 243 calendar days.
 - 285 days passed between releases.
 - 10 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump to 0.15.3 for release ([`8b16ec5`](https://github.com/wasmCloud/wasmCloud/commit/8b16ec5c8dbfa7b776a7360524c7e11c3fa16a32))
    - Update to nuid 0.5 api ([`7757626`](https://github.com/wasmCloud/wasmCloud/commit/77576260a0a3de3771b77c0f2f2a050c2a300ef7))
    - Fix spelling ([`6659528`](https://github.com/wasmCloud/wasmCloud/commit/6659528a4531f8d8024785296a36874b7e409f31))
    - Bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates ([`ef45f59`](https://github.com/wasmCloud/wasmCloud/commit/ef45f597710929d41be989110fc3c51621c9ee62))
    - Bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates ([`c5ba85c`](https://github.com/wasmCloud/wasmCloud/commit/c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33))
    - Enable new component model feature ([`eb9621b`](https://github.com/wasmCloud/wasmCloud/commit/eb9621bddd9febe38b70fae4372ddd74f9031295))
    - Release and update CHANGELOG ([`f128cec`](https://github.com/wasmCloud/wasmCloud/commit/f128cec29f07ae84e37822c5bba1c91eeb9d82fd))
    - Add Host entity type ([`0b164fd`](https://github.com/wasmCloud/wasmCloud/commit/0b164fde352a782a1b3c8a451e5b5bb791505556))
    - Revert "feat(wascap): add Host entity type" ([`a8de756`](https://github.com/wasmCloud/wasmCloud/commit/a8de756cc71eed8e49b4c6dfcbc7d8234020bb66))
    - Add Host entity type ([`add7bb1`](https://github.com/wasmCloud/wasmCloud/commit/add7bb1e11bf76eb235f7aa7b7c6ef7db93bae5e))
    - Replace actor references by component in crates ([`20c72ce`](https://github.com/wasmCloud/wasmCloud/commit/20c72ce0ed423561ae6dbd5a91959bec24ff7cf3))
</details>

## v0.15.2 (2024-11-08)

<csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-f128cec29f07ae84e37822c5bba1c91eeb9d82fd/>

### Chore

 - <csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/> bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, wasmcloud-host v0.22.0, wasmcloud-runtime v0.6.0, wasmcloud-test-util v0.14.0
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests

### New Features

 - <csr-id-0b164fde352a782a1b3c8a451e5b5bb791505556/> add Host entity type
   Add a `Host` entity type to wascap. This allows us to generate JWTs for
   hosts that contains assertions about the metadata they were started
   with. For now this only includes host labels, but this could change in
   the future.
 - <csr-id-add7bb1e11bf76eb235f7aa7b7c6ef7db93bae5e/> add Host entity type
   Add a `Host` entity type to wascap. This allows us to generate JWTs for
   hosts that contains assertions about the metadata they were started
   with. For now this only includes host labels, but this could change in
   the future.

### Bug Fixes

 - <csr-id-eb9621bddd9febe38b70fae4372ddd74f9031295/> enable new component model feature

### Other

 - <csr-id-f128cec29f07ae84e37822c5bba1c91eeb9d82fd/> release and update CHANGELOG

## v0.15.1 (2024-10-23)

<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-f128cec29f07ae84e37822c5bba1c91eeb9d82fd/>

### Chore

 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests

### New Features

 - <csr-id-0b164fde352a782a1b3c8a451e5b5bb791505556/> add Host entity type
   Add a `Host` entity type to wascap. This allows us to generate JWTs for
   hosts that contains assertions about the metadata they were started
   with. For now this only includes host labels, but this could change in
   the future.
 - <csr-id-add7bb1e11bf76eb235f7aa7b7c6ef7db93bae5e/> add Host entity type
   Add a `Host` entity type to wascap. This allows us to generate JWTs for
   hosts that contains assertions about the metadata they were started
   with. For now this only includes host labels, but this could change in
   the future.

### Bug Fixes

 - <csr-id-eb9621bddd9febe38b70fae4372ddd74f9031295/> enable new component model feature

### Other

 - <csr-id-f128cec29f07ae84e37822c5bba1c91eeb9d82fd/> release and update CHANGELOG

## v0.16.0 (2025-03-19)

### Chore

 - <csr-id-3078c88f0ebed96027e20997bccc1c125583fad4/> bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0

### New Features (BREAKING)

 - <csr-id-039d9c839284aa6e379871856d4b17ed1ea29520/> consolidate wash-lib and wash-cli

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release over the course of 12 calendar days.
 - 50 days passed between releases.
 - 2 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0 ([`3078c88`](https://github.com/wasmCloud/wasmCloud/commit/3078c88f0ebed96027e20997bccc1c125583fad4))
    - Consolidate wash-lib and wash-cli ([`039d9c8`](https://github.com/wasmCloud/wasmCloud/commit/039d9c839284aa6e379871856d4b17ed1ea29520))
</details>

## v0.15.0 (2024-06-11)

<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>

### Chore

 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests

### New Features

 - <csr-id-0b164fde352a782a1b3c8a451e5b5bb791505556/> add Host entity type
   Add a `Host` entity type to wascap. This allows us to generate JWTs for
   hosts that contains assertions about the metadata they were started
   with. For now this only includes host labels, but this could change in
   the future.
 - <csr-id-add7bb1e11bf76eb235f7aa7b7c6ef7db93bae5e/> add Host entity type
   Add a `Host` entity type to wascap. This allows us to generate JWTs for
   hosts that contains assertions about the metadata they were started
   with. For now this only includes host labels, but this could change in
   the future.

## v0.14.0 (2024-04-17)

<csr-id-857c9757ebaa5b835a564be5c70ac3466c01c0ca/>
<csr-id-1bad246d9e174384c1a09bdff7e2dc88d911792e/>

### Chore

 - <csr-id-857c9757ebaa5b835a564be5c70ac3466c01c0ca/> bump to 0.14.0
 - <csr-id-1bad246d9e174384c1a09bdff7e2dc88d911792e/> remove unused dependencies

### New Features (BREAKING)

 - <csr-id-3c56e8f18e7e40982c59ee911140cd5965c733f5/> remove capabilities
 - <csr-id-613f660a586c5b65c903161239d5f0388d534a31/> remove capability signing from wascap

### Bug Fixes (BREAKING)

 - <csr-id-93748a1ecd4edd785af257952f1de9497a7ea946/> remove usage of capability signing

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 5 commits contributed to the release over the course of 26 calendar days.
 - 30 days passed between releases.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump to 0.14.0 ([`857c975`](https://github.com/wasmCloud/wasmCloud/commit/857c9757ebaa5b835a564be5c70ac3466c01c0ca))
    - Remove usage of capability signing ([`93748a1`](https://github.com/wasmCloud/wasmCloud/commit/93748a1ecd4edd785af257952f1de9497a7ea946))
    - Remove capabilities ([`3c56e8f`](https://github.com/wasmCloud/wasmCloud/commit/3c56e8f18e7e40982c59ee911140cd5965c733f5))
    - Remove capability signing from wascap ([`613f660`](https://github.com/wasmCloud/wasmCloud/commit/613f660a586c5b65c903161239d5f0388d534a31))
    - Remove unused dependencies ([`1bad246`](https://github.com/wasmCloud/wasmCloud/commit/1bad246d9e174384c1a09bdff7e2dc88d911792e))
</details>

## v0.13.0 (2024-03-17)

<csr-id-36f0b18737f244d3f946faf8a14626dba619b931/>
<csr-id-f5459155f3b96aa67742a8c62eb286cc06885855/>

### Chore

 - <csr-id-36f0b18737f244d3f946faf8a14626dba619b931/> bump to 0.13

### Documentation

 - <csr-id-05ac449d3da207fd495ecbd786220b053fd6300e/> actor to components terminology
   This change only updates documentation terminology
   to use components instead of actors.
   
   Examples will use the terminology components as well so
   I'm opting to rename the example directories now ahead
   of any source code changes for actor to component
   renames.
 - <csr-id-20ffecb027c225fb62d60b584d6b518aff4ceb51/> update wash URLs

### New Features

 - <csr-id-76c1ed7b5c49152aabd83d27f0b8955d7f874864/> support pubsub on wRPC subjects
   Up until now, publishing and subscribing for RPC communcations on the
   NATS cluster happened on subjects that were related to the wasmbus
   protocol (i.e. 'wasmbus.rpc.*').
   
   To support the WIT-native invocations, i.e. wRPC (#1389), we must
   change the publication and subscription subjects to include also the
   subjects that are expected to be used by wprc.
   
   This commit updates the provider-sdk to listen *additionally* to
   subjects that are required/used by wrpc, though we do not yet have an
   implementation for encode/deocde.

### Refactor

 - <csr-id-f5459155f3b96aa67742a8c62eb286cc06885855/> convert lattice-control provider to bindgen
   The `lattice-control` provider (AKA `lattice-controller`) enables
   easy (if not somewhat meta) control of a wasmcloud lattice, using the
   `wasmcloud-control-interface` crate.
   
   While in the past this provider was powered by Smithy contracts, in
   the WIT-ified future we must convert that contract to an WIT-ified
   interface which is backwards compatible with the smithy interface.
   
   This commit converts the `lattice-control` provider to use WIT-ified
   interfaces (rather than Smithy-based interfaces) and `provider-wit-bindgen`.

### New Features (BREAKING)

 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 6 commits contributed to the release over the course of 123 calendar days.
 - 129 days passed between releases.
 - 6 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump to 0.13 ([`36f0b18`](https://github.com/wasmCloud/wasmCloud/commit/36f0b18737f244d3f946faf8a14626dba619b931))
    - Actor to components terminology ([`05ac449`](https://github.com/wasmCloud/wasmCloud/commit/05ac449d3da207fd495ecbd786220b053fd6300e))
    - Support pubsub on wRPC subjects ([`76c1ed7`](https://github.com/wasmCloud/wasmCloud/commit/76c1ed7b5c49152aabd83d27f0b8955d7f874864))
    - Updates topics to the new standard ([`42d069e`](https://github.com/wasmCloud/wasmCloud/commit/42d069eee87d1b5befff1a95b49973064f1a1d1b))
    - Convert lattice-control provider to bindgen ([`f545915`](https://github.com/wasmCloud/wasmCloud/commit/f5459155f3b96aa67742a8c62eb286cc06885855))
    - Update wash URLs ([`20ffecb`](https://github.com/wasmCloud/wasmCloud/commit/20ffecb027c225fb62d60b584d6b518aff4ceb51))
</details>

## v0.12.0 (2023-11-09)

<csr-id-9c8abf3dd1a942f01a70432abb2fb9cfc4d48914/>
<csr-id-ee9d552c7ea1c017d8aa646f64002a85ffebefb8/>
<csr-id-9de9ae3de8799661525b2458303e72cd24cd666f/>
<csr-id-0b59721367d138709b58fa241cdadd4f585203ac/>
<csr-id-171214d4bcffddb9a2a37c2a13fcbed1ec43fd31/>

### Chore

 - <csr-id-9c8abf3dd1a942f01a70432abb2fb9cfc4d48914/> address clippy issues
 - <csr-id-ee9d552c7ea1c017d8aa646f64002a85ffebefb8/> address `clippy` warnings in workspace
 - <csr-id-9de9ae3de8799661525b2458303e72cd24cd666f/> integrate `provider-archive` into the workspace
 - <csr-id-0b59721367d138709b58fa241cdadd4f585203ac/> integrate `wascap` into the workspace

### Refactor

 - <csr-id-171214d4bcffddb9a2a37c2a13fcbed1ec43fd31/> use `OnceLock` to remove `once-cell`
   This commit removes the use of `once-cell` in favor of `std::sync::OnceLock`

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 7 commits contributed to the release over the course of 22 calendar days.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Address clippy issues ([`9c8abf3`](https://github.com/wasmCloud/wasmCloud/commit/9c8abf3dd1a942f01a70432abb2fb9cfc4d48914))
    - Use `OnceLock` to remove `once-cell` ([`171214d`](https://github.com/wasmCloud/wasmCloud/commit/171214d4bcffddb9a2a37c2a13fcbed1ec43fd31))
    - Merge pull request #762 from rvolosatovs/merge/wascap ([`89570cc`](https://github.com/wasmCloud/wasmCloud/commit/89570cc8d7ac7fbf6acd83fdf91f2ac8014d0b77))
    - Address `clippy` warnings in workspace ([`ee9d552`](https://github.com/wasmCloud/wasmCloud/commit/ee9d552c7ea1c017d8aa646f64002a85ffebefb8))
    - Integrate `provider-archive` into the workspace ([`9de9ae3`](https://github.com/wasmCloud/wasmCloud/commit/9de9ae3de8799661525b2458303e72cd24cd666f))
    - Integrate `wascap` into the workspace ([`0b59721`](https://github.com/wasmCloud/wasmCloud/commit/0b59721367d138709b58fa241cdadd4f585203ac))
    - Add 'crates/wascap/' from commit '6dd214c2ea3befb5170d5a711a2eef0f3d14cc09' ([`260ffb0`](https://github.com/wasmCloud/wasmCloud/commit/260ffb029f05b8a6b6f9dcbf6870e281569694c2))
</details>


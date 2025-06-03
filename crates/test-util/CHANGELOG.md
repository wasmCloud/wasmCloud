# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.18.0 (2025-05-28)

### Chore

 - <csr-id-7df6305aac78635e26e478992509bb07915868d9/> bump versions
 - <csr-id-8d9a8ba37f0824e63f42522c111a1e0dfe142047/> Bump nats-server to 2.11.3
 - <csr-id-332f465c5b7902a485de7758250c07a86a00886f/> Add support for overriding Features in WasmCloudTestHost::start_custom
 - <csr-id-3078c88f0ebed96027e20997bccc1c125583fad4/> bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0
 - <csr-id-383fb22ede84002851081ba21f760b35cf9a2263/> Address feedback
 - <csr-id-973ba67e7ebd536c8849179ffa87b306588074ff/> Switch spire-server from testcontainers to local binary
 - <csr-id-5b22019d4844ecb3c760bed89ec196e16770f5b9/> Add nats-jwt-rs and NATS Server testcontainer updates
 - <csr-id-9d9d1c52b260f8fa66140ca9951893b482363a8a/> Add workload identity integration test
 - <csr-id-6659528a4531f8d8024785296a36874b7e409f31/> fix spelling
 - <csr-id-4f30198215220b3f9ce0c2aa6da8aa7d31a6a72d/> bump wasmcloud-core v0.16.0, wash-lib v0.32.0, wash-cli v0.38.0, safety bump 6 crates
   SAFETY BUMP: wash-lib v0.32.0, wash-cli v0.38.0, wasmcloud-host v0.24.0, wasmcloud-provider-sdk v0.13.0, wasmcloud-test-util v0.16.0, wasmcloud-runtime v0.8.0
 - <csr-id-eb52eca817fe24b33e7f1a65c1ba5c46c50bef4e/> removed unused dependencies
   A batch scanning all crates and remove unused dependencies by running 'cargo machete'.
 - <csr-id-609b5a17b312579e32434c7090f2215f8a6a322b/> Enable jetstream by default
 - <csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/> bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, wasmcloud-host v0.22.0, wasmcloud-runtime v0.6.0, wasmcloud-test-util v0.14.0
 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-d8a480bfba3769e56471d408f90d0aaf5a356a4a/> Adopt predefined testcontainers
 - <csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/> update nats-kv version to v1alpha1
 - <csr-id-da879d3e50d32fe1c09edcf2b58cb2db9c9e2661/> update secrets integration to use the update config structure
   Update the secrets integration in a wasmCloud host to include
   information about the policy that determines which backend to
   communicate with. This is a change that comes in from wadm where the
   policy block now contains the information about which backend to use.
   
   This also passes any propertes defined on the policy to the correct
   backend, which are stored as a versioned string-encoded JSON object.
 - <csr-id-d7677a3d1dc1e7a10e49b43c57a6206d4c367f30/> prep for release v0.12.0
   This commit prepares `wasmcloud-test-util` for a release of the next
   "major" version (following semver pre 1.x semantics), version 0.12.0.
 - <csr-id-03433cfbd79ab1b652dd32c6077143fda2379df9/> use NATS 0.33
 - <csr-id-94bfb0e23d4f1f58b70500eaa635717a6ba83484/> partially update to NATS 0.35.1
 - <csr-id-d8b19a210a60e39fbd4a1b9e8cd275116304e7e7/> replace mentions of 'actor' w/ 'component'
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-fa5a77bbf34411340c38bea8ac5975be5af2eeba/> bump to 0.1

### Documentation

 - <csr-id-4eecca55327c1898ef624225e8f26c1d419b62af/> fill out more documentation, tests
   This commit adds documentation and tests for the test util crate, in
   preparation for it being published an replacing the
   `wasmcloud-test-util` that is currently published to crates.io.

### New Features

 - <csr-id-b3693c10e0473603b78fc9fc03772f5051b86f75/> add assert_stop_provider
 - <csr-id-febf47d0ed7d70a245006710960a1a3bd695e6bf/> gate messaging@v3 behind feature flag
 - <csr-id-8b2b1393a96d67dccbef921a757792214904e346/> Add predefined testcontainers
 - <csr-id-df0696e9d5037135fbfb0319bf6b2ae84ae95161/> return link put response
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
 - <csr-id-e39d1cb85011f548a7c20f6ed411ef6a53fe6e34/> Adds tests for static actor config
 - <csr-id-82c249b15dba4dbe4c14a6afd2b52c7d3dc99985/> Glues in named config to actors
   This introduces a new config bundle that can watch for config changes. There
   is probably a way to reduce the number of allocations here, but it is good
   enough for now.
   
   Also, sorry for the new file. I renamed `config.rs` to `host_config.rs` so
   I could reuse the `config.rs` file, but I forgot to git mv. So that file
   hasn't changed
 - <csr-id-9d98442596689e6c7a8896f05365e5ed7a4c4f40/> component-to-component integration test
 - <csr-id-4803b7f2381b5439f862746407ac13a31ebdfee3/> add wasmcloud-test-util crate
   This commit adds a `wasmcloud-test-util` crate, which contains utilities
   for testing wasmCloud hosts, providers, and actors locally

### Bug Fixes

 - <csr-id-9c7eccade1e19875adc03848ca8b6f790c983e7e/> wait for host_started event to ensure ready
 - <csr-id-703b67571b5c35a127e9880cf5a0f1aaa8b353b0/> use NatsHostBuilder for host
 - <csr-id-8cc957a4ee878b3ccc85cf3e7bddcbf5c3ab376a/> ignore auctions for builtin providers when disabled
   This commit enables hosts to ignore auctions for builtin providers if
   they are enabled. While in the past a response to start_provider was
   assumed for every message, we loosen that to return an optional response.
 - <csr-id-221eab623e4e94bf1e337366e0155453ffa64670/> reintroduce joinset for handles
 - <csr-id-2f157ba0aa0d784663d24682361eadb8e7796f97/> enable experimental features in test host
 - <csr-id-1dd07f1f49bbb76d5604956efb7b3fa2443f369b/> squid proxy image
 - <csr-id-e202bb3adfe45ccfc6ef099890f74962263e8f19/> use uppercase SECRET prefix
 - <csr-id-bca30cb58c6f1669894b21d202dae0511a3d6542/> fix reference to http-jsonify-rust component

### Other

 - <csr-id-ef45f597710929d41be989110fc3c51621c9ee62/> bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates
   SAFETY BUMP: wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wash-cli v0.37.0, wasmcloud-host v0.23.0, wasmcloud-runtime v0.7.0, wasmcloud-test-util v0.15.0, wasmcloud-secrets-client v0.6.0
 - <csr-id-869cb024047bb7199ef080c14f327d471dd6e7a2/> wasmcloud-test-util v0.12.0
 - <csr-id-c65d9cab4cc8917eedcad1672812bafad0311ee0/> upgrade to 0.36
 - <csr-id-835b49613f7f0d6903ad53d78f49c17db1e3d90e/> release and update CHANGELOG

### Refactor

 - <csr-id-52ed8befa81bab75817394e1136c196a962f8ca9/> consistently name features
 - <csr-id-30ba77a2253122af3d85e1bce89d70de66cdb283/> reword feature methods
 - <csr-id-035b22eb111ffbe159351a292b13fe6a15681fbb/> return component description from waiting fn
 - <csr-id-9c48f86f7e02273a25324e99824d7d2fe3d7af99/> update for control-interface v2.2.0
 - <csr-id-5ea200a553f79f3da3cb3c21a113ad0a863c2025/> update test util for new ctrl-iface
 - <csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/> include name with secret config
 - <csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/> address feedback, application name optional
 - <csr-id-388662a482442df3f74dfe8f9559fc4c07cedbe5/> collapse application field
 - <csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/> improve error usage of bail
 - <csr-id-4bd1c0bd6a5f338c6c3840b7d96d1143ac2905c6/> wascap::jwt::Actor -> wascap::jwt::Component

### Test

 - <csr-id-7e32b07b59d9b45047d9ed3a202c49104a7f0b73/> add secrets nats kv helpers
 - <csr-id-0f4745b323e3af3b991598ee13b2b166fba74358/> add a test for always deny policy
   This commit adds a basic test for an always-deny policy that ensures
   starting providers and actors fails.
 - <csr-id-8e15d48258489dbb94f83cbea3872d4ee946c70b/> update start_provider with named config

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-bcbb402c2efe3dc881b06e666c70e01e94d3ad72/> rename ctl actor to component

### New Features (BREAKING)

 - <csr-id-089c061be0bf07e6abdeafc17375417eafff4a1b/> support field on SecretRequest
 - <csr-id-98b3986aca562d7f5439d3618d1eaf70f1b7e75a/> add secrets backend topic flag
 - <csr-id-9e23be23131bbcdad746f7e85d33d5812e5f2ff9/> rename actor_scale* events
 - <csr-id-abffe4bac6137371e00c0afa668db907bde082e6/> rename put_link to receive_link_config_as_*
   This commit renames `put_link` which was a part of the
   `ProviderHandler` trait to `receive_link_config_as_target` and
   `receive_link_config_as_source` depending on the position of the
   provider when the link is put.
   
   With both of these explicit methods, users should be able to configure
   their providers appropriately depending on how the link has been put
   to them.
 - <csr-id-4a4b300515e9984a1befe6aaab1a6298d8ea49b1/> wrap all ctl operations in CtlResponse
 - <csr-id-4c54a488f5ea4a7d5f6793db62c9e2b0fd6ddf3a/> wrap all operations in CtlResponse

### Bug Fixes (BREAKING)

 - <csr-id-301ba5aacadfe939db5717eb9cff47a31fffd116/> consistent link operations

### Refactor (BREAKING)

 - <csr-id-6c0b7c463f1a3b60dca0688e3232f67cd1047a4e/> add utilities, improve DX
   This commit adds utility functions to the `wasmcloud-test-util` for
   retrieving randomized ports and waiting until a given HTTP request
   succeeds.
   
   This commit also changes the signatures of many functions in
   wasmcloud-test-util to ensure they can be used flexibly from async
   contexts.
 - <csr-id-e75ca8df4149c40c44ca0cd151f9d5f7d87cb2fa/> replace actor with component

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 71 commits contributed to the release over the course of 442 calendar days.
 - 69 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump versions ([`7df6305`](https://github.com/wasmCloud/wasmCloud/commit/7df6305aac78635e26e478992509bb07915868d9))
    - Bump nats-server to 2.11.3 ([`8d9a8ba`](https://github.com/wasmCloud/wasmCloud/commit/8d9a8ba37f0824e63f42522c111a1e0dfe142047))
    - Wait for host_started event to ensure ready ([`9c7ecca`](https://github.com/wasmCloud/wasmCloud/commit/9c7eccade1e19875adc03848ca8b6f790c983e7e))
    - Use NatsHostBuilder for host ([`703b675`](https://github.com/wasmCloud/wasmCloud/commit/703b67571b5c35a127e9880cf5a0f1aaa8b353b0))
    - Ignore auctions for builtin providers when disabled ([`8cc957a`](https://github.com/wasmCloud/wasmCloud/commit/8cc957a4ee878b3ccc85cf3e7bddcbf5c3ab376a))
    - Add support for overriding Features in WasmCloudTestHost::start_custom ([`332f465`](https://github.com/wasmCloud/wasmCloud/commit/332f465c5b7902a485de7758250c07a86a00886f))
    - Bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0 ([`3078c88`](https://github.com/wasmCloud/wasmCloud/commit/3078c88f0ebed96027e20997bccc1c125583fad4))
    - Address feedback ([`383fb22`](https://github.com/wasmCloud/wasmCloud/commit/383fb22ede84002851081ba21f760b35cf9a2263))
    - Switch spire-server from testcontainers to local binary ([`973ba67`](https://github.com/wasmCloud/wasmCloud/commit/973ba67e7ebd536c8849179ffa87b306588074ff))
    - Add nats-jwt-rs and NATS Server testcontainer updates ([`5b22019`](https://github.com/wasmCloud/wasmCloud/commit/5b22019d4844ecb3c760bed89ec196e16770f5b9))
    - Add workload identity integration test ([`9d9d1c5`](https://github.com/wasmCloud/wasmCloud/commit/9d9d1c52b260f8fa66140ca9951893b482363a8a))
    - Fix spelling ([`6659528`](https://github.com/wasmCloud/wasmCloud/commit/6659528a4531f8d8024785296a36874b7e409f31))
    - Reintroduce joinset for handles ([`221eab6`](https://github.com/wasmCloud/wasmCloud/commit/221eab623e4e94bf1e337366e0155453ffa64670))
    - Add assert_stop_provider ([`b3693c1`](https://github.com/wasmCloud/wasmCloud/commit/b3693c10e0473603b78fc9fc03772f5051b86f75))
    - Bump wasmcloud-core v0.16.0, wash-lib v0.32.0, wash-cli v0.38.0, safety bump 6 crates ([`4f30198`](https://github.com/wasmCloud/wasmCloud/commit/4f30198215220b3f9ce0c2aa6da8aa7d31a6a72d))
    - Gate messaging@v3 behind feature flag ([`febf47d`](https://github.com/wasmCloud/wasmCloud/commit/febf47d0ed7d70a245006710960a1a3bd695e6bf))
    - Consistently name features ([`52ed8be`](https://github.com/wasmCloud/wasmCloud/commit/52ed8befa81bab75817394e1136c196a962f8ca9))
    - Reword feature methods ([`30ba77a`](https://github.com/wasmCloud/wasmCloud/commit/30ba77a2253122af3d85e1bce89d70de66cdb283))
    - Enable experimental features in test host ([`2f157ba`](https://github.com/wasmCloud/wasmCloud/commit/2f157ba0aa0d784663d24682361eadb8e7796f97))
    - Removed unused dependencies ([`eb52eca`](https://github.com/wasmCloud/wasmCloud/commit/eb52eca817fe24b33e7f1a65c1ba5c46c50bef4e))
    - Squid proxy image ([`1dd07f1`](https://github.com/wasmCloud/wasmCloud/commit/1dd07f1f49bbb76d5604956efb7b3fa2443f369b))
    - Bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates ([`ef45f59`](https://github.com/wasmCloud/wasmCloud/commit/ef45f597710929d41be989110fc3c51621c9ee62))
    - Enable jetstream by default ([`609b5a1`](https://github.com/wasmCloud/wasmCloud/commit/609b5a17b312579e32434c7090f2215f8a6a322b))
    - Return component description from waiting fn ([`035b22e`](https://github.com/wasmCloud/wasmCloud/commit/035b22eb111ffbe159351a292b13fe6a15681fbb))
    - Add utilities, improve DX ([`6c0b7c4`](https://github.com/wasmCloud/wasmCloud/commit/6c0b7c463f1a3b60dca0688e3232f67cd1047a4e))
    - Bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates ([`c5ba85c`](https://github.com/wasmCloud/wasmCloud/commit/c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33))
    - Bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates ([`44bf4c8`](https://github.com/wasmCloud/wasmCloud/commit/44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a))
    - Update for control-interface v2.2.0 ([`9c48f86`](https://github.com/wasmCloud/wasmCloud/commit/9c48f86f7e02273a25324e99824d7d2fe3d7af99))
    - Wasmcloud-test-util v0.12.0 ([`869cb02`](https://github.com/wasmCloud/wasmCloud/commit/869cb024047bb7199ef080c14f327d471dd6e7a2))
    - Update test util for new ctrl-iface ([`5ea200a`](https://github.com/wasmCloud/wasmCloud/commit/5ea200a553f79f3da3cb3c21a113ad0a863c2025))
    - Adopt predefined testcontainers ([`d8a480b`](https://github.com/wasmCloud/wasmCloud/commit/d8a480bfba3769e56471d408f90d0aaf5a356a4a))
    - Add predefined testcontainers ([`8b2b139`](https://github.com/wasmCloud/wasmCloud/commit/8b2b1393a96d67dccbef921a757792214904e346))
    - Return link put response ([`df0696e`](https://github.com/wasmCloud/wasmCloud/commit/df0696e9d5037135fbfb0319bf6b2ae84ae95161))
    - Upgrade to 0.36 ([`c65d9ca`](https://github.com/wasmCloud/wasmCloud/commit/c65d9cab4cc8917eedcad1672812bafad0311ee0))
    - Release and update CHANGELOG ([`835b496`](https://github.com/wasmCloud/wasmCloud/commit/835b49613f7f0d6903ad53d78f49c17db1e3d90e))
    - Support field on SecretRequest ([`089c061`](https://github.com/wasmCloud/wasmCloud/commit/089c061be0bf07e6abdeafc17375417eafff4a1b))
    - Include name with secret config ([`c666ef5`](https://github.com/wasmCloud/wasmCloud/commit/c666ef50fecc1ee248bf78d486a915ee077e3b4a))
    - Address feedback, application name optional ([`b56982f`](https://github.com/wasmCloud/wasmCloud/commit/b56982f437209ecaff4fa6946f8fe4c3068a62cd))
    - Collapse application field ([`388662a`](https://github.com/wasmCloud/wasmCloud/commit/388662a482442df3f74dfe8f9559fc4c07cedbe5))
    - Update nats-kv version to v1alpha1 ([`13edb3e`](https://github.com/wasmCloud/wasmCloud/commit/13edb3e395eeb304adb88fcda0ebf1ada2c295c4))
    - Update secrets integration to use the update config structure ([`da879d3`](https://github.com/wasmCloud/wasmCloud/commit/da879d3e50d32fe1c09edcf2b58cb2db9c9e2661))
    - Use uppercase SECRET prefix ([`e202bb3`](https://github.com/wasmCloud/wasmCloud/commit/e202bb3adfe45ccfc6ef099890f74962263e8f19))
    - Prep for release v0.12.0 ([`d7677a3`](https://github.com/wasmCloud/wasmCloud/commit/d7677a3d1dc1e7a10e49b43c57a6206d4c367f30))
    - Improve error usage of bail ([`c30bf33`](https://github.com/wasmCloud/wasmCloud/commit/c30bf33f754c15122ead7f041b7d3e063dd1db33))
    - Add secrets nats kv helpers ([`7e32b07`](https://github.com/wasmCloud/wasmCloud/commit/7e32b07b59d9b45047d9ed3a202c49104a7f0b73))
    - Add secrets backend topic flag ([`98b3986`](https://github.com/wasmCloud/wasmCloud/commit/98b3986aca562d7f5439d3618d1eaf70f1b7e75a))
    - Use NATS 0.33 ([`03433cf`](https://github.com/wasmCloud/wasmCloud/commit/03433cfbd79ab1b652dd32c6077143fda2379df9))
    - Partially update to NATS 0.35.1 ([`94bfb0e`](https://github.com/wasmCloud/wasmCloud/commit/94bfb0e23d4f1f58b70500eaa635717a6ba83484))
    - Bump wascap v0.15.0, wasmcloud-core v0.7.0, wash-lib v0.22.0, wasmcloud-tracing v0.5.0, wasmcloud-provider-sdk v0.6.0, wash-cli v0.29.0, safety bump 5 crates ([`2e38cd4`](https://github.com/wasmCloud/wasmCloud/commit/2e38cd45adef18d47af71b87ca456a25edb2f53a))
    - Fix reference to http-jsonify-rust component ([`bca30cb`](https://github.com/wasmCloud/wasmCloud/commit/bca30cb58c6f1669894b21d202dae0511a3d6542))
    - Bump provider-archive v0.10.1, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0, safety bump 5 crates ([`75a2e52`](https://github.com/wasmCloud/wasmCloud/commit/75a2e52f52690ba143679c90237851ebd07e153f))
    - Replace mentions of 'actor' w/ 'component' ([`d8b19a2`](https://github.com/wasmCloud/wasmCloud/commit/d8b19a210a60e39fbd4a1b9e8cd275116304e7e7))
    - Fill out more documentation, tests ([`4eecca5`](https://github.com/wasmCloud/wasmCloud/commit/4eecca55327c1898ef624225e8f26c1d419b62af))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Replace actor with component ([`e75ca8d`](https://github.com/wasmCloud/wasmCloud/commit/e75ca8df4149c40c44ca0cd151f9d5f7d87cb2fa))
    - Remove cluster_seed/cluster_issuers ([`bc5d296`](https://github.com/wasmCloud/wasmCloud/commit/bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f))
    - Rename ctl actor to component ([`bcbb402`](https://github.com/wasmCloud/wasmCloud/commit/bcbb402c2efe3dc881b06e666c70e01e94d3ad72))
    - Add a test for always deny policy ([`0f4745b`](https://github.com/wasmCloud/wasmCloud/commit/0f4745b323e3af3b991598ee13b2b166fba74358))
    - Rename actor_scale* events ([`9e23be2`](https://github.com/wasmCloud/wasmCloud/commit/9e23be23131bbcdad746f7e85d33d5812e5f2ff9))
    - Wascap::jwt::Actor -> wascap::jwt::Component ([`4bd1c0b`](https://github.com/wasmCloud/wasmCloud/commit/4bd1c0bd6a5f338c6c3840b7d96d1143ac2905c6))
    - Bump to 0.1 ([`fa5a77b`](https://github.com/wasmCloud/wasmCloud/commit/fa5a77bbf34411340c38bea8ac5975be5af2eeba))
    - Rename put_link to receive_link_config_as_* ([`abffe4b`](https://github.com/wasmCloud/wasmCloud/commit/abffe4bac6137371e00c0afa668db907bde082e6))
    - Update start_provider with named config ([`8e15d48`](https://github.com/wasmCloud/wasmCloud/commit/8e15d48258489dbb94f83cbea3872d4ee946c70b))
    - Support pubsub on wRPC subjects ([`76c1ed7`](https://github.com/wasmCloud/wasmCloud/commit/76c1ed7b5c49152aabd83d27f0b8955d7f874864))
    - Adds tests for static actor config ([`e39d1cb`](https://github.com/wasmCloud/wasmCloud/commit/e39d1cb85011f548a7c20f6ed411ef6a53fe6e34))
    - Glues in named config to actors ([`82c249b`](https://github.com/wasmCloud/wasmCloud/commit/82c249b15dba4dbe4c14a6afd2b52c7d3dc99985))
    - Wrap all ctl operations in CtlResponse ([`4a4b300`](https://github.com/wasmCloud/wasmCloud/commit/4a4b300515e9984a1befe6aaab1a6298d8ea49b1))
    - Wrap all operations in CtlResponse ([`4c54a48`](https://github.com/wasmCloud/wasmCloud/commit/4c54a488f5ea4a7d5f6793db62c9e2b0fd6ddf3a))
    - Component-to-component integration test ([`9d98442`](https://github.com/wasmCloud/wasmCloud/commit/9d98442596689e6c7a8896f05365e5ed7a4c4f40))
    - Consistent link operations ([`301ba5a`](https://github.com/wasmCloud/wasmCloud/commit/301ba5aacadfe939db5717eb9cff47a31fffd116))
    - Add wasmcloud-test-util crate ([`4803b7f`](https://github.com/wasmCloud/wasmCloud/commit/4803b7f2381b5439f862746407ac13a31ebdfee3))
</details>

## v0.12.0 (2024-09-30)

<csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/>
<csr-id-da879d3e50d32fe1c09edcf2b58cb2db9c9e2661/>
<csr-id-d7677a3d1dc1e7a10e49b43c57a6206d4c367f30/>
<csr-id-03433cfbd79ab1b652dd32c6077143fda2379df9/>
<csr-id-94bfb0e23d4f1f58b70500eaa635717a6ba83484/>
<csr-id-d8b19a210a60e39fbd4a1b9e8cd275116304e7e7/>
<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-fa5a77bbf34411340c38bea8ac5975be5af2eeba/>
<csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/>
<csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/>
<csr-id-388662a482442df3f74dfe8f9559fc4c07cedbe5/>
<csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/>
<csr-id-4bd1c0bd6a5f338c6c3840b7d96d1143ac2905c6/>
<csr-id-7e32b07b59d9b45047d9ed3a202c49104a7f0b73/>
<csr-id-0f4745b323e3af3b991598ee13b2b166fba74358/>
<csr-id-8e15d48258489dbb94f83cbea3872d4ee946c70b/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-bcbb402c2efe3dc881b06e666c70e01e94d3ad72/>
<csr-id-e75ca8df4149c40c44ca0cd151f9d5f7d87cb2fa/>
<csr-id-5ea200a553f79f3da3cb3c21a113ad0a863c2025/>
<csr-id-c65d9cab4cc8917eedcad1672812bafad0311ee0/>
<csr-id-835b49613f7f0d6903ad53d78f49c17db1e3d90e/>
<csr-id-d8a480bfba3769e56471d408f90d0aaf5a356a4a/>

### Chore

 - <csr-id-13edb3e395eeb304adb88fcda0ebf1ada2c295c4/> update nats-kv version to v1alpha1
 - <csr-id-da879d3e50d32fe1c09edcf2b58cb2db9c9e2661/> update secrets integration to use the update config structure
   Update the secrets integration in a wasmCloud host to include
   information about the policy that determines which backend to
   communicate with. This is a change that comes in from wadm where the
   policy block now contains the information about which backend to use.
   
   This also passes any propertes defined on the policy to the correct
   backend, which are stored as a versioned string-encoded JSON object.
 - <csr-id-d7677a3d1dc1e7a10e49b43c57a6206d4c367f30/> prep for release v0.12.0
   This commit prepares `wasmcloud-test-util` for a release of the next
   "major" version (following semver pre 1.x semantics), version 0.12.0.
 - <csr-id-03433cfbd79ab1b652dd32c6077143fda2379df9/> use NATS 0.33
 - <csr-id-94bfb0e23d4f1f58b70500eaa635717a6ba83484/> partially update to NATS 0.35.1
 - <csr-id-d8b19a210a60e39fbd4a1b9e8cd275116304e7e7/> replace mentions of 'actor' w/ 'component'
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-fa5a77bbf34411340c38bea8ac5975be5af2eeba/> bump to 0.1

### Refactor

 - <csr-id-5ea200a553f79f3da3cb3c21a113ad0a863c2025/> update test util for new ctrl-iface

### Other

 - <csr-id-c65d9cab4cc8917eedcad1672812bafad0311ee0/> upgrade to 0.36
 - <csr-id-835b49613f7f0d6903ad53d78f49c17db1e3d90e/> release and update CHANGELOG

### Chore

 - <csr-id-d8a480bfba3769e56471d408f90d0aaf5a356a4a/> Adopt predefined testcontainers

### Documentation

 - <csr-id-4eecca55327c1898ef624225e8f26c1d419b62af/> fill out more documentation, tests
   This commit adds documentation and tests for the test util crate, in
   preparation for it being published an replacing the
   `wasmcloud-test-util` that is currently published to crates.io.

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
 - <csr-id-e39d1cb85011f548a7c20f6ed411ef6a53fe6e34/> Adds tests for static actor config
 - <csr-id-82c249b15dba4dbe4c14a6afd2b52c7d3dc99985/> Glues in named config to actors
   This introduces a new config bundle that can watch for config changes. There
   is probably a way to reduce the number of allocations here, but it is good
   enough for now.
   
   Also, sorry for the new file. I renamed `config.rs` to `host_config.rs` so
   I could reuse the `config.rs` file, but I forgot to git mv. So that file
   hasn't changed
 - <csr-id-9d98442596689e6c7a8896f05365e5ed7a4c4f40/> component-to-component integration test
 - <csr-id-4803b7f2381b5439f862746407ac13a31ebdfee3/> add wasmcloud-test-util crate
   This commit adds a `wasmcloud-test-util` crate, which contains utilities
   for testing wasmCloud hosts, providers, and actors locally
 - <csr-id-8b2b1393a96d67dccbef921a757792214904e346/> Add predefined testcontainers
 - <csr-id-df0696e9d5037135fbfb0319bf6b2ae84ae95161/> return link put response

### Bug Fixes

 - <csr-id-e202bb3adfe45ccfc6ef099890f74962263e8f19/> use uppercase SECRET prefix
 - <csr-id-bca30cb58c6f1669894b21d202dae0511a3d6542/> fix reference to http-jsonify-rust component

### Refactor

 - <csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/> include name with secret config
 - <csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/> address feedback, application name optional
 - <csr-id-388662a482442df3f74dfe8f9559fc4c07cedbe5/> collapse application field
 - <csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/> improve error usage of bail
 - <csr-id-4bd1c0bd6a5f338c6c3840b7d96d1143ac2905c6/> wascap::jwt::Actor -> wascap::jwt::Component

### Test

 - <csr-id-7e32b07b59d9b45047d9ed3a202c49104a7f0b73/> add secrets nats kv helpers
 - <csr-id-0f4745b323e3af3b991598ee13b2b166fba74358/> add a test for always deny policy
   This commit adds a basic test for an always-deny policy that ensures
   starting providers and actors fails.
 - <csr-id-8e15d48258489dbb94f83cbea3872d4ee946c70b/> update start_provider with named config

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-bcbb402c2efe3dc881b06e666c70e01e94d3ad72/> rename ctl actor to component

### New Features (BREAKING)

 - <csr-id-089c061be0bf07e6abdeafc17375417eafff4a1b/> support field on SecretRequest
 - <csr-id-98b3986aca562d7f5439d3618d1eaf70f1b7e75a/> add secrets backend topic flag
 - <csr-id-9e23be23131bbcdad746f7e85d33d5812e5f2ff9/> rename actor_scale* events
 - <csr-id-abffe4bac6137371e00c0afa668db907bde082e6/> rename put_link to receive_link_config_as_*
   This commit renames `put_link` which was a part of the
   `ProviderHandler` trait to `receive_link_config_as_target` and
   `receive_link_config_as_source` depending on the position of the
   provider when the link is put.
   
   With both of these explicit methods, users should be able to configure
   their providers appropriately depending on how the link has been put
   to them.
 - <csr-id-4a4b300515e9984a1befe6aaab1a6298d8ea49b1/> wrap all ctl operations in CtlResponse
 - <csr-id-4c54a488f5ea4a7d5f6793db62c9e2b0fd6ddf3a/> wrap all operations in CtlResponse

### Bug Fixes (BREAKING)

 - <csr-id-301ba5aacadfe939db5717eb9cff47a31fffd116/> consistent link operations

### Refactor (BREAKING)

 - <csr-id-e75ca8df4149c40c44ca0cd151f9d5f7d87cb2fa/> replace actor with component


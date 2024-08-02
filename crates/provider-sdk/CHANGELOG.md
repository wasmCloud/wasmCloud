# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.7.0 (2024-08-02)

### Chore

 - <csr-id-3097af2824e0fa4477792e798a6c2cda742e3fff/> clippy
 - <csr-id-8c50fa9b90424b12a81276872c6e9b5bda61dd99/> update README
 - <csr-id-929661ae115a05ccdacbcb0eb90642cdd1ff5cea/> enable otel feature for docsrs
 - <csr-id-0b1569b42dccdf3ae0f12e0d93fa9bcedd71e6dc/> address clippy warnings
 - <csr-id-81ab5914e7d08740eb9371c9b718f13f0419c23f/> enable `ring` feature for `async-nats`
 - <csr-id-bd50166619b8810ccdc2bcd80c33ff80d94bc909/> address clippy warnings
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-8c93b0edbf37f1d0b40e065acfafc89af936a425/> bump to 0.4.0
 - <csr-id-902a17ec9bd73e6bf4dc08dca109d7e11765e6e4/> mark `LinkConfig` `non_exhaustive`
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-859663e775f5505ec8fd7ee2bbb2ada73faae0e2/> remove unused braces
 - <csr-id-3d7b64321686139e2e266ff7c69f094bcfac1f6d/> bump to 0.3
 - <csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/> use `&str` directly
 - <csr-id-435300a5d5461860ad5f9abaf2f85cdb6ca3f900/> relax type bounds
 - <csr-id-56e48aaac4a3e11f2f5e98ff2fa136ce9bb2235c/> remove wasmbus rpc client
 - <csr-id-b9770de23b8d3b0fa1adffddb94236403d7e1d3f/> bump `provider-sdk` to 0.2.0
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/> replace error field name with err
 - <csr-id-0023f7e86d5a40a534f623b7220743f27871549e/> reduce verbosity of instrumented functions
 - <csr-id-7b9ad7b57edd06c1c62833965041634811df47eb/> fix format

### New Features

 - <csr-id-c2bb9cb5e2ba1c6b055f6726e86ffc95dab90d2c/> set NATS queue group
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-757c8e06c053030d9c6b39e67cebb4f27786624f/> add Context::link_name
   This commit adds a utility method `link_name` to the `Context` struct
   made available to providers to make it a bit easier to retrieve the
   link name.
   
   Since link names are not included on `Context` natively, the headers
   availble on the context must be searched.
 - <csr-id-e28361935ad3b09d46658488e813c809522317bf/> add support for flame graphs
 - <csr-id-1b076b3479874dbc2f7e575fcee65bab66bd056d/> use `tracing-appender`
   Avoid locking whole process on each logging statement
 - <csr-id-feaf5f9cd63fa7bf8476389396808fac9ba4ce09/> allow providers to access wit metadata for links
   This commit enables providers to easily access WIT meatadata related
   to a link via the `LinkConfig` struct available at link time.
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-e19f77dee5fef7d211965a4a07946a0533bbc4a0/> add macro for propagating context
 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-5f3986365b322624f904260962c3a830580c7b5e/> add trace context to wrpc client
   This commit adds tracing information (if provided) to the wRPC client
   when creating it manually from inside a provider (and when using an
   `InvocationHandler`).
 - <csr-id-8ce845bec7ca3f50e211d36e62fffbb0f36a0b37/> introduce interface provider running utilities
 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk
 - <csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/> use native TLS roots along webpki
 - <csr-id-c610c845e003400cc515c0134cf546f2c7e9f6ac/> add support for init()
   This commit adds support for a implementer-provided provider
   initialization hook that `provider-sdk` can call during
   initialization.
 - <csr-id-614af7e3ed734c56b27cd1d2aacb0789a85e8b81/> implement Redis `wrpc:keyvalue/{atomic,eventual}`
 - <csr-id-e0dac9de4d3a74424e3138971753db9da143db5a/> implement `wasi:http/outgoing-handler` provider
 - <csr-id-e14d0405e9f746041001e101fc24320c9e6b4f9c/> deliver full config with link
 - <csr-id-f50af38d7fb9bda9b8e05703240e6fda55f2c6df/> introduce `run_provider_handler`
 - <csr-id-97aff4f5f93eeb6e31a31e891f742ab252bffe3b/> export wRPC client constructor
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
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
 - <csr-id-5d19ba16a98dca9439628e8449309ccaa763ab10/> change set-target to set-link-name
   Up until the relatively low-level `wasmcloud:bus/lattice` WIT
   interface has used a function called `set-target` to aim invocations
   that occurred in compliant actors and providers.
   
   Since wRPC (#1389)
   enabled  wasmCloud 1.0 is going to be WIT-first going forward, all
   WIT-driven function executions have access to the relevant
   interface (WIT interfaces, rather than Smithy-derived ones) that they
   call, at call time.
   
   Given that actor & provider side function executions have access to
   their WIT interfaces (ex. `wasi:keyvalue/readwrite.get`), what we need
   to do is differentiate between the case where *multiple targets*
   might be responding to the same WIT interface-backed invocations.
   
   Unlike before, `set-target` only needs to really differentiate between *link
   names*.
   
   This commit updates `set-target` to perform differentiate between link
   names, building on the work already done to introduce more opaque
   targeting via Component IDs.
 - <csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/> enable OTEL logs
 - <csr-id-bf396e0cea4dcb5baa0f0cb0201af0fb078f38a5/> update provider bindgen, add kvredis smithy-WIT implementation
 - <csr-id-813ce52a9c11270814eec051dfaa8817bf9f567d/> support chunking and dechunking of requests
 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end
 - <csr-id-c334d84d01b8b92ab9db105f8e6f0c4a6bcef8b1/> send OTEL config via HostData
 - <csr-id-ada90674df5130be6320788bcb08b7868f3b67a5/> add new provider SDK to repo
   This is now manually tested and in a state where I think we should have it
   in the repo. We should be able to keep iterating from there

### Bug Fixes

 - <csr-id-2eb4c9947e0300e86971d398a03f6673de031dc3/> remove configure_observability() call
 - <csr-id-27cb86d9e86b09c2da9e23a4ebfbddf22f3abad2/> wasmcloud messaging provider directionality
 - <csr-id-0f6a1eb97cb46a43c9b24977a8e8dc11061af330/> add messaging triggered test actor
   This commit is the culmination of a few things that were required for
   getting our flavor of E2E tests (in the top level `tests/` dir)
   working for a Provider & Actor.
   
   This commit is quite large because it does many things:
   
   - Adds missing implementation to bindgen for provider -> actor invocations
   - Uncomments implementation from the host for wasmcloud:messaging
   - Adds an invoker component that reacts to messaging rather than HTTP
   - Uses messaging & keyvalue providers plus the actor in a single test
   
   With this, we have an easy to understand way to test every provider
   that we have in the repository.
 - <csr-id-2dbc392215c9fa1971b1f3bd83fab0807c60aaee/> use send_request to handle request timeout
 - <csr-id-07d818cdbd50ae350d236fb1cc309d86b75739ea/> add what clippy took from me
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-c604aca1db1017e2458cf66eab232b081d615521/> enable `ansi` feature

### Other

 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-f324674facf892f5db1747d1b780ccd22383a940/> clarify timeout purpose
 - <csr-id-4adbf0647f1ef987e92fbf927db9d09e64d3ecd8/> update `async-nats` to 0.33
 - <csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/> 'upstream/main' into `merge/wash`
 - <csr-id-d98a317b30e352ea0d73439ad3fa790ddfb8bf3f/> update opentelemetry

### Refactor

 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-8e92e3b292e72af232524577c3410891a749eca2/> rename secrets redis provider
 - <csr-id-d8ad4376cb4db282047de8c4f62f6b8b907c9356/> improve error representations, cleanup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-1814fd52e32ed4286d8f0be838a6c525cfeccc30/> enable consumer groups with kafka provider
   This commit enables consumer groups via link time configuration in the
   kafka provider.
   
   The native Rust ecosystem for Kafka is somewhat immature, so to do
   this we must switch to a different crate
   `kafka-rs` (https://crates.io/crates/kafka), which implements consumer
   groups but is otherwise missing some features.
   
   With this branch, one can specify CONSUMER_GROUP in link configuration
   to control the consumer group that is used for Kafka consumers (whose
   incoming messages will trigger downstream components).
 - <csr-id-2406601fb476e6a8ab7f7b2617ab70834474891a/> introduce `serve_provider_exports`
 - <csr-id-1dcbeee35e180ed0334c48b2dc80c9c15ad51994/> update observability init code
 - <csr-id-7c664a88cd7bbaa201b9b07a2bb9ba1c215a3b56/> minor changes from pr feedback
 - <csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/> return wrapped `WrpcClient` directly
 - <csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/> remove `ProviderHandler`
 - <csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/> move wasmbus RPC topic generation to core
   This commit moves the topic generation functions that were used for
   wasmbus RPC topics from `provider-sdk` to `core` so that they can be
   used/referred to more widely.
 - <csr-id-54321c7cce159b7dad073dfc254dd4f13c21d2a2/> remove `serialize` and `deserialize`
 - <csr-id-637be5dea8c8bef72f6f76ccc673477b7b0f1d0f/> minimize API surface
 - <csr-id-05ae20c8ef474ad2249c6ad4b6ca8cc3b7d01b01/> subscribe for control subjects in init
 - <csr-id-2e473aa8b3337179566c71a9a93a945519b467db/> export subject constructors
 - <csr-id-68dadeddb79cc041851d2adcfeb0417a4006d296/> extract reusable `init_provider`
 - <csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/> get values from new link def constistently
   This commit updates all providers that were marked with TODOs related
   to using named config to use `InterfaceLinkDefinition.target_config`
   temporarily.
   
   The idea is to stuff configuration into the "names"s of configs that
   were supposed to be in use (i.e. `["NAME=VALUE", "NAME=VALUE"]` rather
   than `["config-1", "config-2"]`), ahead of named config being ready.
 - <csr-id-aea0a282911a704ee0d70ad38f267d8d8cc00d78/> convert blobstore-fs to bindgen
 - <csr-id-0319a9245589709d96b03786374d8026beb5d5d0/> move chunking to core
 - <csr-id-6f0a7d848e49d4cdc66dffe38fd8b41657f32649/> simply re-export wasmcloud_core as core
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers
 - <csr-id-3430c72b11564acc0624987cd3df08c629d7d197/> remove `atty` dependency

### Style

 - <csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/> replace needs_chunking function with direct comparison

### Test

 - <csr-id-8e15d48258489dbb94f83cbea3872d4ee946c70b/> update start_provider with named config

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers

### New Features (BREAKING)

 - <csr-id-4095f2ead49ffd61a8bc57189ccbd8a7defa4de0/> handle hostdata xkeys, decrypt link secrets
 - <csr-id-8ad2cde49cb52872af4c9753be7c422092ae56ee/> add trace_level option
 - <csr-id-f643d51e1fd01209ad93e93a04eed66b268dc2e2/> handle secrets at init and link
 - <csr-id-88aedb17e90011cb602f48845c3896a3d836c980/> support storing directional links
 - <csr-id-abffe4bac6137371e00c0afa668db907bde082e6/> rename put_link to receive_link_config_as_*
   This commit renames `put_link` which was a part of the
   `ProviderHandler` trait to `receive_link_config_as_target` and
   `receive_link_config_as_source` depending on the position of the
   provider when the link is put.
   
   With both of these explicit methods, users should be able to configure
   their providers appropriately depending on how the link has been put
   to them.

### Bug Fixes (BREAKING)

 - <csr-id-903955009340190283c813fa225bae514fb15c03/> rename actor to component

### Refactor (BREAKING)

 - <csr-id-e1e50d7366716b61ddce52244e3dd66758ee0b82/> remove link_name, rename provider_key
 - <csr-id-e75d3e2f2da91371266715723a3229b2138bf4f9/> unflatten provider errors & invocation errors
   This commit refactors areas of provider code (SDK, in-tree providers)
   that previously used `ProviderInvocationError`s which mixed
   `InvocationError`s and a string-based catch-all for provider-internal
   errors to return types that are true to the WIT contracts.
   
   With this commit, provider developers must code to the interface that
   matches the WIT contract (ex. `async fn operation() ->
   T`), rather than having values that are wrapped in `ProviderInvocationResult`.
   
   Contracts that were ported/not originally written with failure in mind
   (i.e. not using `result<_,_>` in WIT) should be rewritten (in the
   future) for operations that may fail, rather than relying on the
   previously used `ProviderInvocation(Result|Error)` structures.
 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice
 - <csr-id-5fd0557c7ff454211e3f590333ff4dda208a1f7a/> make publish method crate-public
 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 106 commits contributed to the release over the course of 373 calendar days.
 - 100 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump for test-util release ([`7cd2e71`](https://github.com/wasmCloud/wasmCloud/commit/7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4))
    - Clippy ([`3097af2`](https://github.com/wasmCloud/wasmCloud/commit/3097af2824e0fa4477792e798a6c2cda742e3fff))
    - Update README ([`8c50fa9`](https://github.com/wasmCloud/wasmCloud/commit/8c50fa9b90424b12a81276872c6e9b5bda61dd99))
    - Efficiency, pass optional vec secrets ([`cfbf232`](https://github.com/wasmCloud/wasmCloud/commit/cfbf23226f34f3e7245a5d36cd7bb15e1796850c))
    - Rename secrets redis provider ([`8e92e3b`](https://github.com/wasmCloud/wasmCloud/commit/8e92e3b292e72af232524577c3410891a749eca2))
    - Improve error representations, cleanup ([`d8ad437`](https://github.com/wasmCloud/wasmCloud/commit/d8ad4376cb4db282047de8c4f62f6b8b907c9356))
    - Handle hostdata xkeys, decrypt link secrets ([`4095f2e`](https://github.com/wasmCloud/wasmCloud/commit/4095f2ead49ffd61a8bc57189ccbd8a7defa4de0))
    - Remove extra trace_level field ([`4e1d6da`](https://github.com/wasmCloud/wasmCloud/commit/4e1d6da189ff49790d876cd244aed89114efba98))
    - Add trace_level option ([`8ad2cde`](https://github.com/wasmCloud/wasmCloud/commit/8ad2cde49cb52872af4c9753be7c422092ae56ee))
    - Handle secrets at init and link ([`f643d51`](https://github.com/wasmCloud/wasmCloud/commit/f643d51e1fd01209ad93e93a04eed66b268dc2e2))
    - Enable consumer groups with kafka provider ([`1814fd5`](https://github.com/wasmCloud/wasmCloud/commit/1814fd52e32ed4286d8f0be838a6c525cfeccc30))
    - Enable otel feature for docsrs ([`929661a`](https://github.com/wasmCloud/wasmCloud/commit/929661ae115a05ccdacbcb0eb90642cdd1ff5cea))
    - Address clippy warnings ([`0b1569b`](https://github.com/wasmCloud/wasmCloud/commit/0b1569b42dccdf3ae0f12e0d93fa9bcedd71e6dc))
    - Clarify timeout purpose ([`f324674`](https://github.com/wasmCloud/wasmCloud/commit/f324674facf892f5db1747d1b780ccd22383a940))
    - Introduce `serve_provider_exports` ([`2406601`](https://github.com/wasmCloud/wasmCloud/commit/2406601fb476e6a8ab7f7b2617ab70834474891a))
    - Set NATS queue group ([`c2bb9cb`](https://github.com/wasmCloud/wasmCloud/commit/c2bb9cb5e2ba1c6b055f6726e86ffc95dab90d2c))
    - Enable `ring` feature for `async-nats` ([`81ab591`](https://github.com/wasmCloud/wasmCloud/commit/81ab5914e7d08740eb9371c9b718f13f0419c23f))
    - Address clippy warnings ([`bd50166`](https://github.com/wasmCloud/wasmCloud/commit/bd50166619b8810ccdc2bcd80c33ff80d94bc909))
    - Upgrade `wrpc`, `async-nats`, `wasmtime` ([`9cb1b78`](https://github.com/wasmCloud/wasmCloud/commit/9cb1b784fe7a8892d73bdb40d1172b1879fcd932))
    - Update observability init code ([`1dcbeee`](https://github.com/wasmCloud/wasmCloud/commit/1dcbeee35e180ed0334c48b2dc80c9c15ad51994))
    - Remove configure_observability() call ([`2eb4c99`](https://github.com/wasmCloud/wasmCloud/commit/2eb4c9947e0300e86971d398a03f6673de031dc3))
    - Add Context::link_name ([`757c8e0`](https://github.com/wasmCloud/wasmCloud/commit/757c8e06c053030d9c6b39e67cebb4f27786624f))
    - Add support for flame graphs ([`e283619`](https://github.com/wasmCloud/wasmCloud/commit/e28361935ad3b09d46658488e813c809522317bf))
    - Use `tracing-appender` ([`1b076b3`](https://github.com/wasmCloud/wasmCloud/commit/1b076b3479874dbc2f7e575fcee65bab66bd056d))
    - Bump wasmcloud-tracing v0.5.0, wasmcloud-provider-sdk v0.6.0, wash-cli v0.29.0 ([`b22d338`](https://github.com/wasmCloud/wasmCloud/commit/b22d338d0d61f8a438c4d6ea5e8e5cd26116ade5))
    - Bump wascap v0.15.0, wasmcloud-core v0.7.0, wash-lib v0.22.0, wasmcloud-tracing v0.5.0, wasmcloud-provider-sdk v0.6.0, wash-cli v0.29.0, safety bump 5 crates ([`2e38cd4`](https://github.com/wasmCloud/wasmCloud/commit/2e38cd45adef18d47af71b87ca456a25edb2f53a))
    - Allow providers to access wit metadata for links ([`feaf5f9`](https://github.com/wasmCloud/wasmCloud/commit/feaf5f9cd63fa7bf8476389396808fac9ba4ce09))
    - Bump provider-archive v0.10.2, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0 ([`73c0ef0`](https://github.com/wasmCloud/wasmCloud/commit/73c0ef0bbe2f6b525655939d2cd30740aef4b6bc))
    - Bump provider-archive v0.10.1, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0, safety bump 5 crates ([`75a2e52`](https://github.com/wasmCloud/wasmCloud/commit/75a2e52f52690ba143679c90237851ebd07e153f))
    - Generate changelogs after 1.0.1 release ([`4e0313a`](https://github.com/wasmCloud/wasmCloud/commit/4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e))
    - Remove link_name, rename provider_key ([`e1e50d7`](https://github.com/wasmCloud/wasmCloud/commit/e1e50d7366716b61ddce52244e3dd66758ee0b82))
    - Support storing directional links ([`88aedb1`](https://github.com/wasmCloud/wasmCloud/commit/88aedb17e90011cb602f48845c3896a3d836c980))
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - Minor changes from pr feedback ([`7c664a8`](https://github.com/wasmCloud/wasmCloud/commit/7c664a88cd7bbaa201b9b07a2bb9ba1c215a3b56))
    - Wasmcloud messaging provider directionality ([`27cb86d`](https://github.com/wasmCloud/wasmCloud/commit/27cb86d9e86b09c2da9e23a4ebfbddf22f3abad2))
    - Add macro for propagating context ([`e19f77d`](https://github.com/wasmCloud/wasmCloud/commit/e19f77dee5fef7d211965a4a07946a0533bbc4a0))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Bump to 0.4.0 ([`8c93b0e`](https://github.com/wasmCloud/wasmCloud/commit/8c93b0edbf37f1d0b40e065acfafc89af936a425))
    - Rename actor to component ([`9039550`](https://github.com/wasmCloud/wasmCloud/commit/903955009340190283c813fa225bae514fb15c03))
    - Update `wrpc:keyvalue` in providers ([`9cd2b40`](https://github.com/wasmCloud/wasmCloud/commit/9cd2b4034f8d5688ce250429dc14120eaf61b483))
    - Remove cluster_seed/cluster_issuers ([`bc5d296`](https://github.com/wasmCloud/wasmCloud/commit/bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f))
    - Mark `LinkConfig` `non_exhaustive` ([`902a17e`](https://github.com/wasmCloud/wasmCloud/commit/902a17ec9bd73e6bf4dc08dca109d7e11765e6e4))
    - Return wrapped `WrpcClient` directly ([`87eb6c8`](https://github.com/wasmCloud/wasmCloud/commit/87eb6c8b2c0bd31def1cfdc6121c612c4dc90871))
    - Switch to `wit-bindgen-wrpc` ([`322f471`](https://github.com/wasmCloud/wasmCloud/commit/322f471f9a8154224a50ec33517c9f5b1716d2d5))
    - Remove `ProviderHandler` ([`8082135`](https://github.com/wasmCloud/wasmCloud/commit/8082135282f66b5d56fe6d14bb5ce6dc510d4b63))
    - Excises vestigal remains of wasmbus-rpc ([`fd69df4`](https://github.com/wasmCloud/wasmCloud/commit/fd69df40f24ca565ace0f8c97a0c47a89db575a4))
    - Add trace context to wrpc client ([`5f39863`](https://github.com/wasmCloud/wasmCloud/commit/5f3986365b322624f904260962c3a830580c7b5e))
    - Introduce interface provider running utilities ([`8ce845b`](https://github.com/wasmCloud/wasmCloud/commit/8ce845bec7ca3f50e211d36e62fffbb0f36a0b37))
    - Introduce provider interface sdk ([`a84492d`](https://github.com/wasmCloud/wasmCloud/commit/a84492d15d154a272de33680f6338379fc036a3a))
    - Use native TLS roots along webpki ([`07b5e70`](https://github.com/wasmCloud/wasmCloud/commit/07b5e70a7f1321d184962d7197a8d98d1ecaaf71))
    - Move wasmbus RPC topic generation to core ([`5d73831`](https://github.com/wasmCloud/wasmCloud/commit/5d7383137897d28a1bc5df9b1c48f75281dab55b))
    - Remove unused braces ([`859663e`](https://github.com/wasmCloud/wasmCloud/commit/859663e775f5505ec8fd7ee2bbb2ada73faae0e2))
    - Add support for init() ([`c610c84`](https://github.com/wasmCloud/wasmCloud/commit/c610c845e003400cc515c0134cf546f2c7e9f6ac))
    - Bump to 0.3 ([`3d7b643`](https://github.com/wasmCloud/wasmCloud/commit/3d7b64321686139e2e266ff7c69f094bcfac1f6d))
    - Implement Redis `wrpc:keyvalue/{atomic,eventual}` ([`614af7e`](https://github.com/wasmCloud/wasmCloud/commit/614af7e3ed734c56b27cd1d2aacb0789a85e8b81))
    - Implement `wasi:http/outgoing-handler` provider ([`e0dac9d`](https://github.com/wasmCloud/wasmCloud/commit/e0dac9de4d3a74424e3138971753db9da143db5a))
    - Deliver full config with link ([`e14d040`](https://github.com/wasmCloud/wasmCloud/commit/e14d0405e9f746041001e101fc24320c9e6b4f9c))
    - Rename put_link to receive_link_config_as_* ([`abffe4b`](https://github.com/wasmCloud/wasmCloud/commit/abffe4bac6137371e00c0afa668db907bde082e6))
    - Update start_provider with named config ([`8e15d48`](https://github.com/wasmCloud/wasmCloud/commit/8e15d48258489dbb94f83cbea3872d4ee946c70b))
    - Remove `serialize` and `deserialize` ([`54321c7`](https://github.com/wasmCloud/wasmCloud/commit/54321c7cce159b7dad073dfc254dd4f13c21d2a2))
    - Minimize API surface ([`637be5d`](https://github.com/wasmCloud/wasmCloud/commit/637be5dea8c8bef72f6f76ccc673477b7b0f1d0f))
    - Introduce `run_provider_handler` ([`f50af38`](https://github.com/wasmCloud/wasmCloud/commit/f50af38d7fb9bda9b8e05703240e6fda55f2c6df))
    - Export wRPC client constructor ([`97aff4f`](https://github.com/wasmCloud/wasmCloud/commit/97aff4f5f93eeb6e31a31e891f742ab252bffe3b))
    - Subscribe for control subjects in init ([`05ae20c`](https://github.com/wasmCloud/wasmCloud/commit/05ae20c8ef474ad2249c6ad4b6ca8cc3b7d01b01))
    - Export subject constructors ([`2e473aa`](https://github.com/wasmCloud/wasmCloud/commit/2e473aa8b3337179566c71a9a93a945519b467db))
    - Use `&str` directly ([`6b369d4`](https://github.com/wasmCloud/wasmCloud/commit/6b369d49cd37a87dca1f92f31c4d4d3e33dec501))
    - Relax type bounds ([`435300a`](https://github.com/wasmCloud/wasmCloud/commit/435300a5d5461860ad5f9abaf2f85cdb6ca3f900))
    - Remove wasmbus rpc client ([`56e48aa`](https://github.com/wasmCloud/wasmCloud/commit/56e48aaac4a3e11f2f5e98ff2fa136ce9bb2235c))
    - Extract reusable `init_provider` ([`68daded`](https://github.com/wasmCloud/wasmCloud/commit/68dadeddb79cc041851d2adcfeb0417a4006d296))
    - Switch to using --enable-observability and --enable-<signal> flags ([`868570b`](https://github.com/wasmCloud/wasmCloud/commit/868570be8d94a6d73608c7cde5d2422e15f9eb0c))
    - Add messaging triggered test actor ([`0f6a1eb`](https://github.com/wasmCloud/wasmCloud/commit/0f6a1eb97cb46a43c9b24977a8e8dc11061af330))
    - Get values from new link def constistently ([`b6a6b04`](https://github.com/wasmCloud/wasmCloud/commit/b6a6b04229730d6783c3fee61c6e078cd3b962ef))
    - Support pubsub on wRPC subjects ([`76c1ed7`](https://github.com/wasmCloud/wasmCloud/commit/76c1ed7b5c49152aabd83d27f0b8955d7f874864))
    - Change set-target to set-link-name ([`5d19ba1`](https://github.com/wasmCloud/wasmCloud/commit/5d19ba16a98dca9439628e8449309ccaa763ab10))
    - Use send_request to handle request timeout ([`2dbc392`](https://github.com/wasmCloud/wasmCloud/commit/2dbc392215c9fa1971b1f3bd83fab0807c60aaee))
    - Enable OTEL logs ([`3602bdf`](https://github.com/wasmCloud/wasmCloud/commit/3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3))
    - Unflatten provider errors & invocation errors ([`e75d3e2`](https://github.com/wasmCloud/wasmCloud/commit/e75d3e2f2da91371266715723a3229b2138bf4f9))
    - Rename lattice prefix to just lattice ([`6e8faab`](https://github.com/wasmCloud/wasmCloud/commit/6e8faab6a6e9f9bb7327ffb71ded2a83718920f7))
    - Bump `provider-sdk` to 0.2.0 ([`b9770de`](https://github.com/wasmCloud/wasmCloud/commit/b9770de23b8d3b0fa1adffddb94236403d7e1d3f))
    - Make publish method crate-public ([`5fd0557`](https://github.com/wasmCloud/wasmCloud/commit/5fd0557c7ff454211e3f590333ff4dda208a1f7a))
    - Update `async-nats` to 0.33 ([`4adbf06`](https://github.com/wasmCloud/wasmCloud/commit/4adbf0647f1ef987e92fbf927db9d09e64d3ecd8))
    - Add descriptions to crates ([`cb0bcab`](https://github.com/wasmCloud/wasmCloud/commit/cb0bcab822cb4290c673051ec1dd98d034a61546))
    - 'upstream/main' into `merge/wash` ([`0f967b0`](https://github.com/wasmCloud/wasmCloud/commit/0f967b065f30a0b5418f7ed519fdef3dc75a6205))
    - Convert blobstore-fs to bindgen ([`aea0a28`](https://github.com/wasmCloud/wasmCloud/commit/aea0a282911a704ee0d70ad38f267d8d8cc00d78))
    - Replace error field name with err ([`3ffbd3a`](https://github.com/wasmCloud/wasmCloud/commit/3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa))
    - Update provider bindgen, add kvredis smithy-WIT implementation ([`bf396e0`](https://github.com/wasmCloud/wasmCloud/commit/bf396e0cea4dcb5baa0f0cb0201af0fb078f38a5))
    - Reduce verbosity of instrumented functions ([`0023f7e`](https://github.com/wasmCloud/wasmCloud/commit/0023f7e86d5a40a534f623b7220743f27871549e))
    - Add cfg block to import ([`a810769`](https://github.com/wasmCloud/wasmCloud/commit/a810769b7be36f02443b707ca1ae06c1e8bf33cc))
    - Add what clippy took from me ([`07d818c`](https://github.com/wasmCloud/wasmCloud/commit/07d818cdbd50ae350d236fb1cc309d86b75739ea))
    - Fix format ([`7b9ad7b`](https://github.com/wasmCloud/wasmCloud/commit/7b9ad7b57edd06c1c62833965041634811df47eb))
    - Attach traces on inbound and outbound messages ([`74142c4`](https://github.com/wasmCloud/wasmCloud/commit/74142c4cff683565fb321b7b65fbb158b5a9c990))
    - Make content_length a required field ([`6428747`](https://github.com/wasmCloud/wasmCloud/commit/642874717b6aab760d4692f9e8b12803548314e2))
    - Replace needs_chunking function with direct comparison ([`6de67aa`](https://github.com/wasmCloud/wasmCloud/commit/6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06))
    - Support chunking and dechunking of requests ([`813ce52`](https://github.com/wasmCloud/wasmCloud/commit/813ce52a9c11270814eec051dfaa8817bf9f567d))
    - Move chunking to core ([`0319a92`](https://github.com/wasmCloud/wasmCloud/commit/0319a9245589709d96b03786374d8026beb5d5d0))
    - Simply re-export wasmcloud_core as core ([`6f0a7d8`](https://github.com/wasmCloud/wasmCloud/commit/6f0a7d848e49d4cdc66dffe38fd8b41657f32649))
    - Replace lazy_static with once_cell ([`e1d7356`](https://github.com/wasmCloud/wasmCloud/commit/e1d7356bb0a07af9f4e6b1626f5df33709f3ed78))
    - Construct a strongly typed HostData to send to providers ([`23f1759`](https://github.com/wasmCloud/wasmCloud/commit/23f1759e818117f007df8d9b1bdfdfa7710c98c5))
    - Support OTEL traces end-to-end ([`675d364`](https://github.com/wasmCloud/wasmCloud/commit/675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6))
    - Send OTEL config via HostData ([`c334d84`](https://github.com/wasmCloud/wasmCloud/commit/c334d84d01b8b92ab9db105f8e6f0c4a6bcef8b1))
    - Update opentelemetry ([`d98a317`](https://github.com/wasmCloud/wasmCloud/commit/d98a317b30e352ea0d73439ad3fa790ddfb8bf3f))
    - Enable `ansi` feature ([`c604aca`](https://github.com/wasmCloud/wasmCloud/commit/c604aca1db1017e2458cf66eab232b081d615521))
    - Remove `atty` dependency ([`3430c72`](https://github.com/wasmCloud/wasmCloud/commit/3430c72b11564acc0624987cd3df08c629d7d197))
    - Merge pull request #396 from rvolosatovs/feat/provider-sdk ([`6ed04f0`](https://github.com/wasmCloud/wasmCloud/commit/6ed04f00a335333196f6bafb96f2c40155537df3))
    - Add new provider SDK to repo ([`ada9067`](https://github.com/wasmCloud/wasmCloud/commit/ada90674df5130be6320788bcb08b7868f3b67a5))
</details>

## 0.6.0 (2024-06-12)

<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-8c93b0edbf37f1d0b40e065acfafc89af936a425/>
<csr-id-902a17ec9bd73e6bf4dc08dca109d7e11765e6e4/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-859663e775f5505ec8fd7ee2bbb2ada73faae0e2/>
<csr-id-3d7b64321686139e2e266ff7c69f094bcfac1f6d/>
<csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/>
<csr-id-435300a5d5461860ad5f9abaf2f85cdb6ca3f900/>
<csr-id-56e48aaac4a3e11f2f5e98ff2fa136ce9bb2235c/>
<csr-id-b9770de23b8d3b0fa1adffddb94236403d7e1d3f/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/>
<csr-id-0023f7e86d5a40a534f623b7220743f27871549e/>
<csr-id-7b9ad7b57edd06c1c62833965041634811df47eb/>
<csr-id-4adbf0647f1ef987e92fbf927db9d09e64d3ecd8/>
<csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/>
<csr-id-d98a317b30e352ea0d73439ad3fa790ddfb8bf3f/>
<csr-id-7c664a88cd7bbaa201b9b07a2bb9ba1c215a3b56/>
<csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/>
<csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/>
<csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/>
<csr-id-54321c7cce159b7dad073dfc254dd4f13c21d2a2/>
<csr-id-637be5dea8c8bef72f6f76ccc673477b7b0f1d0f/>
<csr-id-05ae20c8ef474ad2249c6ad4b6ca8cc3b7d01b01/>
<csr-id-2e473aa8b3337179566c71a9a93a945519b467db/>
<csr-id-68dadeddb79cc041851d2adcfeb0417a4006d296/>
<csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/>
<csr-id-aea0a282911a704ee0d70ad38f267d8d8cc00d78/>
<csr-id-0319a9245589709d96b03786374d8026beb5d5d0/>
<csr-id-6f0a7d848e49d4cdc66dffe38fd8b41657f32649/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-3430c72b11564acc0624987cd3df08c629d7d197/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-8e15d48258489dbb94f83cbea3872d4ee946c70b/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-e1e50d7366716b61ddce52244e3dd66758ee0b82/>
<csr-id-e75d3e2f2da91371266715723a3229b2138bf4f9/>
<csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/>
<csr-id-5fd0557c7ff454211e3f590333ff4dda208a1f7a/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>

### Chore

 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-8c93b0edbf37f1d0b40e065acfafc89af936a425/> bump to 0.4.0
 - <csr-id-902a17ec9bd73e6bf4dc08dca109d7e11765e6e4/> mark `LinkConfig` `non_exhaustive`
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-859663e775f5505ec8fd7ee2bbb2ada73faae0e2/> remove unused braces
 - <csr-id-3d7b64321686139e2e266ff7c69f094bcfac1f6d/> bump to 0.3
 - <csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/> use `&str` directly
 - <csr-id-435300a5d5461860ad5f9abaf2f85cdb6ca3f900/> relax type bounds
 - <csr-id-56e48aaac4a3e11f2f5e98ff2fa136ce9bb2235c/> remove wasmbus rpc client
 - <csr-id-b9770de23b8d3b0fa1adffddb94236403d7e1d3f/> bump `provider-sdk` to 0.2.0
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/> replace error field name with err
 - <csr-id-0023f7e86d5a40a534f623b7220743f27871549e/> reduce verbosity of instrumented functions
 - <csr-id-7b9ad7b57edd06c1c62833965041634811df47eb/> fix format

### New Features

 - <csr-id-feaf5f9cd63fa7bf8476389396808fac9ba4ce09/> allow providers to access wit metadata for links
   This commit enables providers to easily access WIT meatadata related
   to a link via the `LinkConfig` struct available at link time.
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-e19f77dee5fef7d211965a4a07946a0533bbc4a0/> add macro for propagating context
 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-5f3986365b322624f904260962c3a830580c7b5e/> add trace context to wrpc client
   This commit adds tracing information (if provided) to the wRPC client
   when creating it manually from inside a provider (and when using an
   `InvocationHandler`).
 - <csr-id-8ce845bec7ca3f50e211d36e62fffbb0f36a0b37/> introduce interface provider running utilities
 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk
 - <csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/> use native TLS roots along webpki
 - <csr-id-c610c845e003400cc515c0134cf546f2c7e9f6ac/> add support for init()
   This commit adds support for a implementer-provided provider
   initialization hook that `provider-sdk` can call during
   initialization.
 - <csr-id-614af7e3ed734c56b27cd1d2aacb0789a85e8b81/> implement Redis `wrpc:keyvalue/{atomic,eventual}`
 - <csr-id-e0dac9de4d3a74424e3138971753db9da143db5a/> implement `wasi:http/outgoing-handler` provider
 - <csr-id-e14d0405e9f746041001e101fc24320c9e6b4f9c/> deliver full config with link
 - <csr-id-f50af38d7fb9bda9b8e05703240e6fda55f2c6df/> introduce `run_provider_handler`
 - <csr-id-97aff4f5f93eeb6e31a31e891f742ab252bffe3b/> export wRPC client constructor
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
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
 - <csr-id-5d19ba16a98dca9439628e8449309ccaa763ab10/> change set-target to set-link-name
   Up until the relatively low-level `wasmcloud:bus/lattice` WIT
   interface has used a function called `set-target` to aim invocations
   that occurred in compliant actors and providers.
   
   Since wRPC (#1389)
   enabled  wasmCloud 1.0 is going to be WIT-first going forward, all
   WIT-driven function executions have access to the relevant
   interface (WIT interfaces, rather than Smithy-derived ones) that they
   call, at call time.
   
   Given that actor & provider side function executions have access to
   their WIT interfaces (ex. `wasi:keyvalue/readwrite.get`), what we need
   to do is differentiate between the case where *multiple targets*
   might be responding to the same WIT interface-backed invocations.
   
   Unlike before, `set-target` only needs to really differentiate between *link
   names*.
   
   This commit updates `set-target` to perform differentiate between link
   names, building on the work already done to introduce more opaque
   targeting via Component IDs.
 - <csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/> enable OTEL logs
 - <csr-id-bf396e0cea4dcb5baa0f0cb0201af0fb078f38a5/> update provider bindgen, add kvredis smithy-WIT implementation
 - <csr-id-813ce52a9c11270814eec051dfaa8817bf9f567d/> support chunking and dechunking of requests
 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end
 - <csr-id-c334d84d01b8b92ab9db105f8e6f0c4a6bcef8b1/> send OTEL config via HostData
 - <csr-id-ada90674df5130be6320788bcb08b7868f3b67a5/> add new provider SDK to repo
   This is now manually tested and in a state where I think we should have it
   in the repo. We should be able to keep iterating from there

### Bug Fixes

<csr-id-2dbc392215c9fa1971b1f3bd83fab0807c60aaee/>
<csr-id-07d818cdbd50ae350d236fb1cc309d86b75739ea/>
<csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/>
<csr-id-c604aca1db1017e2458cf66eab232b081d615521/>

 - <csr-id-27cb86d9e86b09c2da9e23a4ebfbddf22f3abad2/> wasmcloud messaging provider directionality
 - <csr-id-0f6a1eb97cb46a43c9b24977a8e8dc11061af330/> add messaging triggered test actor
   This commit is the culmination of a few things that were required for
   getting our flavor of E2E tests (in the top level `tests/` dir)
   working for a Provider & Actor.
   
   This commit is quite large because it does many things:
   
   - Adds missing implementation to bindgen for provider -> actor invocations

### Other

 - <csr-id-4adbf0647f1ef987e92fbf927db9d09e64d3ecd8/> update `async-nats` to 0.33
 - <csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/> 'upstream/main' into `merge/wash`
 - <csr-id-d98a317b30e352ea0d73439ad3fa790ddfb8bf3f/> update opentelemetry

### Refactor

 - <csr-id-7c664a88cd7bbaa201b9b07a2bb9ba1c215a3b56/> minor changes from pr feedback
 - <csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/> return wrapped `WrpcClient` directly
 - <csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/> remove `ProviderHandler`
 - <csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/> move wasmbus RPC topic generation to core
   This commit moves the topic generation functions that were used for
   wasmbus RPC topics from `provider-sdk` to `core` so that they can be
   used/referred to more widely.
 - <csr-id-54321c7cce159b7dad073dfc254dd4f13c21d2a2/> remove `serialize` and `deserialize`
 - <csr-id-637be5dea8c8bef72f6f76ccc673477b7b0f1d0f/> minimize API surface
 - <csr-id-05ae20c8ef474ad2249c6ad4b6ca8cc3b7d01b01/> subscribe for control subjects in init
 - <csr-id-2e473aa8b3337179566c71a9a93a945519b467db/> export subject constructors
 - <csr-id-68dadeddb79cc041851d2adcfeb0417a4006d296/> extract reusable `init_provider`
 - <csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/> get values from new link def constistently
   This commit updates all providers that were marked with TODOs related
   to using named config to use `InterfaceLinkDefinition.target_config`
   temporarily.
   
   The idea is to stuff configuration into the "names"s of configs that
   were supposed to be in use (i.e. `["NAME=VALUE", "NAME=VALUE"]` rather
   than `["config-1", "config-2"]`), ahead of named config being ready.
 - <csr-id-aea0a282911a704ee0d70ad38f267d8d8cc00d78/> convert blobstore-fs to bindgen
 - <csr-id-0319a9245589709d96b03786374d8026beb5d5d0/> move chunking to core
 - <csr-id-6f0a7d848e49d4cdc66dffe38fd8b41657f32649/> simply re-export wasmcloud_core as core
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers
 - <csr-id-3430c72b11564acc0624987cd3df08c629d7d197/> remove `atty` dependency

### Style

 - <csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/> replace needs_chunking function with direct comparison

### Test

 - <csr-id-8e15d48258489dbb94f83cbea3872d4ee946c70b/> update start_provider with named config

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers

### New Features (BREAKING)

 - <csr-id-88aedb17e90011cb602f48845c3896a3d836c980/> support storing directional links
 - <csr-id-abffe4bac6137371e00c0afa668db907bde082e6/> rename put_link to receive_link_config_as_*
   This commit renames `put_link` which was a part of the
   `ProviderHandler` trait to `receive_link_config_as_target` and
   `receive_link_config_as_source` depending on the position of the
   provider when the link is put.
   
   With both of these explicit methods, users should be able to configure
   their providers appropriately depending on how the link has been put
   to them.

### Bug Fixes (BREAKING)

 - <csr-id-903955009340190283c813fa225bae514fb15c03/> rename actor to component

### Refactor (BREAKING)

 - <csr-id-e1e50d7366716b61ddce52244e3dd66758ee0b82/> remove link_name, rename provider_key
 - <csr-id-e75d3e2f2da91371266715723a3229b2138bf4f9/> unflatten provider errors & invocation errors
   This commit refactors areas of provider code (SDK, in-tree providers)
   that previously used `ProviderInvocationError`s which mixed
   `InvocationError`s and a string-based catch-all for provider-internal
   errors to return types that are true to the WIT contracts.
   
   With this commit, provider developers must code to the interface that
   matches the WIT contract (ex. `async fn operation() ->
   T`), rather than having values that are wrapped in `ProviderInvocationResult`.
   
   Contracts that were ported/not originally written with failure in mind
   (i.e. not using `result<_,_>` in WIT) should be rewritten (in the
   future) for operations that may fail, rather than relying on the
   previously used `ProviderInvocation(Result|Error)` structures.
 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice
 - <csr-id-5fd0557c7ff454211e3f590333ff4dda208a1f7a/> make publish method crate-public
 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field

<csr-unknown>
Uncomments implementation from the host for wasmcloud:messagingAdds an invoker component that reacts to messaging rather than HTTPUses messaging & keyvalue providers plus the actor in a single test<csr-unknown/>

## 0.5.0 (2024-05-08)

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-8c93b0edbf37f1d0b40e065acfafc89af936a425/>
<csr-id-902a17ec9bd73e6bf4dc08dca109d7e11765e6e4/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-859663e775f5505ec8fd7ee2bbb2ada73faae0e2/>
<csr-id-3d7b64321686139e2e266ff7c69f094bcfac1f6d/>
<csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/>
<csr-id-435300a5d5461860ad5f9abaf2f85cdb6ca3f900/>
<csr-id-56e48aaac4a3e11f2f5e98ff2fa136ce9bb2235c/>
<csr-id-b9770de23b8d3b0fa1adffddb94236403d7e1d3f/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/>
<csr-id-0023f7e86d5a40a534f623b7220743f27871549e/>
<csr-id-7b9ad7b57edd06c1c62833965041634811df47eb/>
<csr-id-4adbf0647f1ef987e92fbf927db9d09e64d3ecd8/>
<csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/>
<csr-id-d98a317b30e352ea0d73439ad3fa790ddfb8bf3f/>
<csr-id-7c664a88cd7bbaa201b9b07a2bb9ba1c215a3b56/>
<csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/>
<csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/>
<csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/>
<csr-id-54321c7cce159b7dad073dfc254dd4f13c21d2a2/>
<csr-id-637be5dea8c8bef72f6f76ccc673477b7b0f1d0f/>
<csr-id-05ae20c8ef474ad2249c6ad4b6ca8cc3b7d01b01/>
<csr-id-2e473aa8b3337179566c71a9a93a945519b467db/>
<csr-id-68dadeddb79cc041851d2adcfeb0417a4006d296/>
<csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/>
<csr-id-aea0a282911a704ee0d70ad38f267d8d8cc00d78/>
<csr-id-0319a9245589709d96b03786374d8026beb5d5d0/>
<csr-id-6f0a7d848e49d4cdc66dffe38fd8b41657f32649/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-3430c72b11564acc0624987cd3df08c629d7d197/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-8e15d48258489dbb94f83cbea3872d4ee946c70b/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-e75d3e2f2da91371266715723a3229b2138bf4f9/>
<csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/>
<csr-id-5fd0557c7ff454211e3f590333ff4dda208a1f7a/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>
<csr-id-e1e50d7366716b61ddce52244e3dd66758ee0b82/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-8c93b0edbf37f1d0b40e065acfafc89af936a425/> bump to 0.4.0
 - <csr-id-902a17ec9bd73e6bf4dc08dca109d7e11765e6e4/> mark `LinkConfig` `non_exhaustive`
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-859663e775f5505ec8fd7ee2bbb2ada73faae0e2/> remove unused braces
 - <csr-id-3d7b64321686139e2e266ff7c69f094bcfac1f6d/> bump to 0.3
 - <csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/> use `&str` directly
 - <csr-id-435300a5d5461860ad5f9abaf2f85cdb6ca3f900/> relax type bounds
 - <csr-id-56e48aaac4a3e11f2f5e98ff2fa136ce9bb2235c/> remove wasmbus rpc client
 - <csr-id-b9770de23b8d3b0fa1adffddb94236403d7e1d3f/> bump `provider-sdk` to 0.2.0
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/> replace error field name with err
 - <csr-id-0023f7e86d5a40a534f623b7220743f27871549e/> reduce verbosity of instrumented functions
 - <csr-id-7b9ad7b57edd06c1c62833965041634811df47eb/> fix format

### Chore

 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release

### Refactor (BREAKING)

 - <csr-id-e1e50d7366716b61ddce52244e3dd66758ee0b82/> remove link_name, rename provider_key

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features

### New Features

 - <csr-id-e19f77dee5fef7d211965a4a07946a0533bbc4a0/> add macro for propagating context
 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-5f3986365b322624f904260962c3a830580c7b5e/> add trace context to wrpc client
   This commit adds tracing information (if provided) to the wRPC client
   when creating it manually from inside a provider (and when using an
   `InvocationHandler`).
 - <csr-id-8ce845bec7ca3f50e211d36e62fffbb0f36a0b37/> introduce interface provider running utilities
 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk
 - <csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/> use native TLS roots along webpki
 - <csr-id-c610c845e003400cc515c0134cf546f2c7e9f6ac/> add support for init()
   This commit adds support for a implementer-provided provider
   initialization hook that `provider-sdk` can call during
   initialization.
 - <csr-id-614af7e3ed734c56b27cd1d2aacb0789a85e8b81/> implement Redis `wrpc:keyvalue/{atomic,eventual}`
 - <csr-id-e0dac9de4d3a74424e3138971753db9da143db5a/> implement `wasi:http/outgoing-handler` provider
 - <csr-id-e14d0405e9f746041001e101fc24320c9e6b4f9c/> deliver full config with link
 - <csr-id-f50af38d7fb9bda9b8e05703240e6fda55f2c6df/> introduce `run_provider_handler`
 - <csr-id-97aff4f5f93eeb6e31a31e891f742ab252bffe3b/> export wRPC client constructor
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
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
 - <csr-id-5d19ba16a98dca9439628e8449309ccaa763ab10/> change set-target to set-link-name
   Up until the relatively low-level `wasmcloud:bus/lattice` WIT
   interface has used a function called `set-target` to aim invocations
   that occurred in compliant actors and providers.
   
   Since wRPC (#1389)
   enabled  wasmCloud 1.0 is going to be WIT-first going forward, all
   WIT-driven function executions have access to the relevant
   interface (WIT interfaces, rather than Smithy-derived ones) that they
   call, at call time.
   
   Given that actor & provider side function executions have access to
   their WIT interfaces (ex. `wasi:keyvalue/readwrite.get`), what we need
   to do is differentiate between the case where *multiple targets*
   might be responding to the same WIT interface-backed invocations.
   
   Unlike before, `set-target` only needs to really differentiate between *link
   names*.
   
   This commit updates `set-target` to perform differentiate between link
   names, building on the work already done to introduce more opaque
   targeting via Component IDs.
 - <csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/> enable OTEL logs
 - <csr-id-bf396e0cea4dcb5baa0f0cb0201af0fb078f38a5/> update provider bindgen, add kvredis smithy-WIT implementation
 - <csr-id-813ce52a9c11270814eec051dfaa8817bf9f567d/> support chunking and dechunking of requests
 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end
 - <csr-id-c334d84d01b8b92ab9db105f8e6f0c4a6bcef8b1/> send OTEL config via HostData
 - <csr-id-ada90674df5130be6320788bcb08b7868f3b67a5/> add new provider SDK to repo
   This is now manually tested and in a state where I think we should have it
   in the repo. We should be able to keep iterating from there
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

### Bug Fixes

<csr-id-2dbc392215c9fa1971b1f3bd83fab0807c60aaee/>
<csr-id-07d818cdbd50ae350d236fb1cc309d86b75739ea/>
<csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/>
<csr-id-c604aca1db1017e2458cf66eab232b081d615521/>

 - <csr-id-27cb86d9e86b09c2da9e23a4ebfbddf22f3abad2/> wasmcloud messaging provider directionality
 - <csr-id-0f6a1eb97cb46a43c9b24977a8e8dc11061af330/> add messaging triggered test actor
   This commit is the culmination of a few things that were required for
   getting our flavor of E2E tests (in the top level `tests/` dir)
   working for a Provider & Actor.
   
   This commit is quite large because it does many things:
   
   - Adds missing implementation to bindgen for provider -> actor invocations

### Other

 - <csr-id-4adbf0647f1ef987e92fbf927db9d09e64d3ecd8/> update `async-nats` to 0.33
 - <csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/> 'upstream/main' into `merge/wash`
 - <csr-id-d98a317b30e352ea0d73439ad3fa790ddfb8bf3f/> update opentelemetry

### Refactor

 - <csr-id-7c664a88cd7bbaa201b9b07a2bb9ba1c215a3b56/> minor changes from pr feedback
 - <csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/> return wrapped `WrpcClient` directly
 - <csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/> remove `ProviderHandler`
 - <csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/> move wasmbus RPC topic generation to core
   This commit moves the topic generation functions that were used for
   wasmbus RPC topics from `provider-sdk` to `core` so that they can be
   used/referred to more widely.
 - <csr-id-54321c7cce159b7dad073dfc254dd4f13c21d2a2/> remove `serialize` and `deserialize`
 - <csr-id-637be5dea8c8bef72f6f76ccc673477b7b0f1d0f/> minimize API surface
 - <csr-id-05ae20c8ef474ad2249c6ad4b6ca8cc3b7d01b01/> subscribe for control subjects in init
 - <csr-id-2e473aa8b3337179566c71a9a93a945519b467db/> export subject constructors
 - <csr-id-68dadeddb79cc041851d2adcfeb0417a4006d296/> extract reusable `init_provider`
 - <csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/> get values from new link def constistently
   This commit updates all providers that were marked with TODOs related
   to using named config to use `InterfaceLinkDefinition.target_config`
   temporarily.
   
   The idea is to stuff configuration into the "names"s of configs that
   were supposed to be in use (i.e. `["NAME=VALUE", "NAME=VALUE"]` rather
   than `["config-1", "config-2"]`), ahead of named config being ready.
 - <csr-id-aea0a282911a704ee0d70ad38f267d8d8cc00d78/> convert blobstore-fs to bindgen
 - <csr-id-0319a9245589709d96b03786374d8026beb5d5d0/> move chunking to core
 - <csr-id-6f0a7d848e49d4cdc66dffe38fd8b41657f32649/> simply re-export wasmcloud_core as core
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers
 - <csr-id-3430c72b11564acc0624987cd3df08c629d7d197/> remove `atty` dependency

### Style

 - <csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/> replace needs_chunking function with direct comparison

### Test

 - <csr-id-8e15d48258489dbb94f83cbea3872d4ee946c70b/> update start_provider with named config

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers

### New Features (BREAKING)

 - <csr-id-abffe4bac6137371e00c0afa668db907bde082e6/> rename put_link to receive_link_config_as_*
   This commit renames `put_link` which was a part of the
   `ProviderHandler` trait to `receive_link_config_as_target` and
   `receive_link_config_as_source` depending on the position of the
   provider when the link is put.
   
   With both of these explicit methods, users should be able to configure
   their providers appropriately depending on how the link has been put
   to them.
 - <csr-id-88aedb17e90011cb602f48845c3896a3d836c980/> support storing directional links

### Bug Fixes (BREAKING)

 - <csr-id-903955009340190283c813fa225bae514fb15c03/> rename actor to component

### Refactor (BREAKING)

 - <csr-id-e75d3e2f2da91371266715723a3229b2138bf4f9/> unflatten provider errors & invocation errors
   This commit refactors areas of provider code (SDK, in-tree providers)
   that previously used `ProviderInvocationError`s which mixed
   `InvocationError`s and a string-based catch-all for provider-internal
   errors to return types that are true to the WIT contracts.
   
   With this commit, provider developers must code to the interface that
   matches the WIT contract (ex. `async fn operation() ->
   T`), rather than having values that are wrapped in `ProviderInvocationResult`.
   
   Contracts that were ported/not originally written with failure in mind
   (i.e. not using `result<_,_>` in WIT) should be rewritten (in the
   future) for operations that may fail, rather than relying on the
   previously used `ProviderInvocation(Result|Error)` structures.
 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice
 - <csr-id-5fd0557c7ff454211e3f590333ff4dda208a1f7a/> make publish method crate-public
 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field


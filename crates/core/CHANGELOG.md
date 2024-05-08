# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.6.0 (2024-05-08)

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-95233cbade898a8b17df2fec3d6aed8ce8ceca2a/>
<csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-0d9002340ca8776c92a7d1e8b2caa4f804bb1bfb/>
<csr-id-eb0599fbdc6e1ac58616c7676b89bf7b19d4c662/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-8e071dde1a98caa7339e92882bb63c433ae2a042/>
<csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/>
<csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/>
<csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/>
<csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/>
<csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/>
<csr-id-c654448653db224c6a676ecf43150d880a9daf8c/>
<csr-id-c49a6ef0b6460b3eb463315fe31878eb71ae5364/>
<csr-id-123e53611e6d0b2bd4e92358783213784653fbf6/>
<csr-id-7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb/>
<csr-id-0319a9245589709d96b03786374d8026beb5d5d0/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/>
<csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-95233cbade898a8b17df2fec3d6aed8ce8ceca2a/> bump to 0.3
 - <csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/> use `&str` directly
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-0d9002340ca8776c92a7d1e8b2caa4f804bb1bfb/> move CallTargetInterface to core
 - <csr-id-eb0599fbdc6e1ac58616c7676b89bf7b19d4c662/> address clippy issues
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-8e071dde1a98caa7339e92882bb63c433ae2a042/> remove direct `wasmbus_rpc` dependency
 - <csr-id-3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa/> replace error field name with err

### Chore

 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features

### New Features

 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk
 - <csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/> use native TLS roots along webpki
 - <csr-id-614af7e3ed734c56b27cd1d2aacb0789a85e8b81/> implement Redis `wrpc:keyvalue/{atomic,eventual}`
 - <csr-id-e0dac9de4d3a74424e3138971753db9da143db5a/> implement `wasi:http/outgoing-handler` provider
 - <csr-id-e14d0405e9f746041001e101fc24320c9e6b4f9c/> deliver full config with link
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
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
 - <csr-id-813ce52a9c11270814eec051dfaa8817bf9f567d/> support chunking and dechunking of requests
 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

### Bug Fixes

 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-5ed5367063e39f890dabafdc476ea2370d32aae7/> remove LatticeTargetId
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Other

 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC

### Refactor

 - <csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/> return wrapped `WrpcClient` directly
 - <csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/> move wasmbus RPC topic generation to core
   This commit moves the topic generation functions that were used for
   wasmbus RPC topics from `provider-sdk` to `core` so that they can be
   used/referred to more widely.
 - <csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/> get values from new link def constistently
   This commit updates all providers that were marked with TODOs related
   to using named config to use `InterfaceLinkDefinition.target_config`
   temporarily.
   
   The idea is to stuff configuration into the "names"s of configs that
   were supposed to be in use (i.e. `["NAME=VALUE", "NAME=VALUE"]` rather
   than `["config-1", "config-2"]`), ahead of named config being ready.
 - <csr-id-c654448653db224c6a676ecf43150d880a9daf8c/> move wasmcloud wrpc transport client to core
   This commit moves the wasmcloud-specific wrpc transport client to the
   `wasmcloud-core` crate. From here, it can be used by both
   the host (`wasmbus`) and other places like tests.
 - <csr-id-c49a6ef0b6460b3eb463315fe31878eb71ae5364/> InterfaceLinkDefinition -> core
   This commit refators the types defined in both `wasmcloud-core` and
   `wasmcloud-control-interface` to make it easier to distinguish what
   types belong where and what they're related to.
   
   Ultimately the goal here was was to move `InterfaceLinkDefinition`
   into `wasmcloud-core` so it can be used in other places, but it was a
   good chance to reorganize.
 - <csr-id-123e53611e6d0b2bd4e92358783213784653fbf6/> convert httpclient provider to bindgen
   This commit converts the in-tree httpclient provider to use
   provider-wit-bindgen for it's implementation.
 - <csr-id-7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb/> clean-up imports
 - <csr-id-0319a9245589709d96b03786374d8026beb5d5d0/> move chunking to core

### Style

 - <csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/> replace needs_chunking function with direct comparison

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/> remove LinkDefinition
 - <csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/> bump to 0.2.0 for async-nats release

### New Features (BREAKING)

 - <csr-id-3f2d2f44470d44809fb83de2fa34b29ad1e6cb30/> Adds version to control API
   This should be the final breaking change of the API and it will require
   a two phased rollout. I'll need to cut new core and host versions first
   and then update wash to use the new host for tests.
 - <csr-id-7fbd597546c0ae25d5ce981b716167e4cc01263c/> pass config directly to providers
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Refactor (BREAKING)

 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 48 commits contributed to the release over the course of 253 calendar days.
 - 47 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Generate changelogs after 1.0.1 release ([`4e0313a`](https://github.com/wasmCloud/wasmCloud/commit/4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e))
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Remove cluster_seed/cluster_issuers ([`bc5d296`](https://github.com/wasmCloud/wasmCloud/commit/bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f))
    - Return wrapped `WrpcClient` directly ([`87eb6c8`](https://github.com/wasmCloud/wasmCloud/commit/87eb6c8b2c0bd31def1cfdc6121c612c4dc90871))
    - Excises vestigal remains of wasmbus-rpc ([`fd69df4`](https://github.com/wasmCloud/wasmCloud/commit/fd69df40f24ca565ace0f8c97a0c47a89db575a4))
    - Remove LinkDefinition ([`8e7d6c8`](https://github.com/wasmCloud/wasmCloud/commit/8e7d6c80b56e143bb09dc441e8b21104328d0ab0))
    - Adds version to control API ([`3f2d2f4`](https://github.com/wasmCloud/wasmCloud/commit/3f2d2f44470d44809fb83de2fa34b29ad1e6cb30))
    - Introduce provider interface sdk ([`a84492d`](https://github.com/wasmCloud/wasmCloud/commit/a84492d15d154a272de33680f6338379fc036a3a))
    - Use native TLS roots along webpki ([`07b5e70`](https://github.com/wasmCloud/wasmCloud/commit/07b5e70a7f1321d184962d7197a8d98d1ecaaf71))
    - Move wasmbus RPC topic generation to core ([`5d73831`](https://github.com/wasmCloud/wasmCloud/commit/5d7383137897d28a1bc5df9b1c48f75281dab55b))
    - Fix `link_name` functionality, reorganize tests ([`4ed3891`](https://github.com/wasmCloud/wasmCloud/commit/4ed38913f19fcd4dd44dfdcc9007e80e80cdc960))
    - Bump to 0.3 ([`95233cb`](https://github.com/wasmCloud/wasmCloud/commit/95233cbade898a8b17df2fec3d6aed8ce8ceca2a))
    - Implement Redis `wrpc:keyvalue/{atomic,eventual}` ([`614af7e`](https://github.com/wasmCloud/wasmCloud/commit/614af7e3ed734c56b27cd1d2aacb0789a85e8b81))
    - Implement `wasi:http/outgoing-handler` provider ([`e0dac9d`](https://github.com/wasmCloud/wasmCloud/commit/e0dac9de4d3a74424e3138971753db9da143db5a))
    - Deliver full config with link ([`e14d040`](https://github.com/wasmCloud/wasmCloud/commit/e14d0405e9f746041001e101fc24320c9e6b4f9c))
    - Update wRPC ([`95cfb6d`](https://github.com/wasmCloud/wasmCloud/commit/95cfb6d99f0e54243b2fb2618de39210d8f3694f))
    - Pass config directly to providers ([`7fbd597`](https://github.com/wasmCloud/wasmCloud/commit/7fbd597546c0ae25d5ce981b716167e4cc01263c))
    - Remove LatticeTargetId ([`5ed5367`](https://github.com/wasmCloud/wasmCloud/commit/5ed5367063e39f890dabafdc476ea2370d32aae7))
    - Use `&str` directly ([`6b369d4`](https://github.com/wasmCloud/wasmCloud/commit/6b369d49cd37a87dca1f92f31c4d4d3e33dec501))
    - Use traces instead of tracing user-facing language to align with OTEL signal names ([`d65512b`](https://github.com/wasmCloud/wasmCloud/commit/d65512b5e86eb4d13e64cffa220a5a842c7bb72b))
    - Add flags for overriding the default OpenTelemetry endpoint ([`6fe14b8`](https://github.com/wasmCloud/wasmCloud/commit/6fe14b89d4c26e5c01e54773268c6d0f04236e71))
    - Switch to using --enable-observability and --enable-<signal> flags ([`868570b`](https://github.com/wasmCloud/wasmCloud/commit/868570be8d94a6d73608c7cde5d2422e15f9eb0c))
    - Move CallTargetInterface to core ([`0d90023`](https://github.com/wasmCloud/wasmCloud/commit/0d9002340ca8776c92a7d1e8b2caa4f804bb1bfb))
    - Correct comment on wrpc Client ([`dc2c93d`](https://github.com/wasmCloud/wasmCloud/commit/dc2c93df97bb119bb2a024d5bd3458394f421792))
    - Get values from new link def constistently ([`b6a6b04`](https://github.com/wasmCloud/wasmCloud/commit/b6a6b04229730d6783c3fee61c6e078cd3b962ef))
    - Move wasmcloud wrpc transport client to core ([`c654448`](https://github.com/wasmCloud/wasmCloud/commit/c654448653db224c6a676ecf43150d880a9daf8c))
    - Support pubsub on wRPC subjects ([`76c1ed7`](https://github.com/wasmCloud/wasmCloud/commit/76c1ed7b5c49152aabd83d27f0b8955d7f874864))
    - InterfaceLinkDefinition -> core ([`c49a6ef`](https://github.com/wasmCloud/wasmCloud/commit/c49a6ef0b6460b3eb463315fe31878eb71ae5364))
    - Change set-target to set-link-name ([`5d19ba1`](https://github.com/wasmCloud/wasmCloud/commit/5d19ba16a98dca9439628e8449309ccaa763ab10))
    - Updates topics to the new standard ([`42d069e`](https://github.com/wasmCloud/wasmCloud/commit/42d069eee87d1b5befff1a95b49973064f1a1d1b))
    - Bump to 0.2.0 for async-nats release ([`6abbcac`](https://github.com/wasmCloud/wasmCloud/commit/6abbcac954a9834d871ea69b8a40bd79d258c0f1))
    - Convert httpclient provider to bindgen ([`123e536`](https://github.com/wasmCloud/wasmCloud/commit/123e53611e6d0b2bd4e92358783213784653fbf6))
    - Address clippy issues ([`eb0599f`](https://github.com/wasmCloud/wasmCloud/commit/eb0599fbdc6e1ac58616c7676b89bf7b19d4c662))
    - Clean-up imports ([`7402a1f`](https://github.com/wasmCloud/wasmCloud/commit/7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb))
    - Add descriptions to crates ([`cb0bcab`](https://github.com/wasmCloud/wasmCloud/commit/cb0bcab822cb4290c673051ec1dd98d034a61546))
    - Remove direct `wasmbus_rpc` dependency ([`8e071dd`](https://github.com/wasmCloud/wasmCloud/commit/8e071dde1a98caa7339e92882bb63c433ae2a042))
    - Replace error field name with err ([`3ffbd3a`](https://github.com/wasmCloud/wasmCloud/commit/3ffbd3ae2770a2bb7ef2d5635489e2725b3d9daa))
    - Allow namespaces with slashes ([`1829b27`](https://github.com/wasmCloud/wasmCloud/commit/1829b27213e836cb347a542e9cdc771c74427892))
    - Include context on host errors ([`0e6e2da`](https://github.com/wasmCloud/wasmCloud/commit/0e6e2da7720e469b85940cadde3756b2afd64f7c))
    - Look for invocation responses from providers ([`7502bcb`](https://github.com/wasmCloud/wasmCloud/commit/7502bcb569420e2d402bf66d8a5eff2e6481a80b))
    - Enable `std` anyhow feature ([`a896f05`](https://github.com/wasmCloud/wasmCloud/commit/a896f05a35824f5e2ba16fdb1c1f5217c52a5388))
    - Make content_length a required field ([`6428747`](https://github.com/wasmCloud/wasmCloud/commit/642874717b6aab760d4692f9e8b12803548314e2))
    - Replace needs_chunking function with direct comparison ([`6de67aa`](https://github.com/wasmCloud/wasmCloud/commit/6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06))
    - Support chunking and dechunking of requests ([`813ce52`](https://github.com/wasmCloud/wasmCloud/commit/813ce52a9c11270814eec051dfaa8817bf9f567d))
    - Move chunking to core ([`0319a92`](https://github.com/wasmCloud/wasmCloud/commit/0319a9245589709d96b03786374d8026beb5d5d0))
    - Support OTEL traces end-to-end ([`675d364`](https://github.com/wasmCloud/wasmCloud/commit/675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6))
</details>


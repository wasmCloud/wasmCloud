# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.15.0 (2024-11-08)

### Chore

 - <csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/> bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, wasmcloud-host v0.22.0, wasmcloud-runtime v0.6.0, wasmcloud-test-util v0.14.0
 - <csr-id-db94b15b82f041bd80026c6f3dcf5e5102701e38/> Improve parse_wit_package_name handling
 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-b8d229303bc1f8d1e0983cb5066f7b08bd961bbc/> Revert OtelProtocol rename, add future compatibility aliasing
 - <csr-id-ebe8ba9c7984a158c2c7e787bf02a420be62c530/> Use Default impl for Level
 - <csr-id-c205148b7f67ab5e80edbae46489083fcb665f99/> remove redundant `tower` dep
 - <csr-id-d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba/> Switch oci-distribution to oci feature
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
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

### Documentation

 - <csr-id-48f6307da226a48969d3d08188db89d3d8069495/> add README and update docs

### New Features

 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-ea814a1603d9d2ac7173c729024ba2834d97f45b/> fix tests, update parsing of advanced WIT package names
 - <csr-id-02d88655045d7e620c2452b7d7689cede4ad12db/> add RPC subject for provider config updates
 - <csr-id-4ffee2ed95985902071cbdbf8300dba8e2c37d81/> add string and byte utility functions for SecretValue
   This commit add some utility functions to enable easily accessing
   string values or byte vector values of `SecretValue`s
 - <csr-id-10e5d702d940a4c36dff542d21c6f56f6c7cb28f/> impl Zeroize for secret values
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/> fetch secrets for providers and links
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
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

### Bug Fixes

 - <csr-id-86863ee2ed2e0bb8c2c39844baff5cb8a17119fd/> allow missing registry authentication
   This commit fixes a bug that ignored the `allow_insecure` setting when
   building registry configuration.
 - <csr-id-842b1c0f24c78ab5b891be204675748750387424/> prep for upgrade to rustls_native_certs v0.8.0
 - <csr-id-21d0601b066a29a8b8f182c26372a0adeea290eb/> add missing feature for `oci-wasm`
 - <csr-id-56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c/> Add signal-specific path components to OtelConfig's default endpoints
 - <csr-id-825ef3a28cbdf49727b902a0a8d5e43aa502c522/> default to http otel protocol if not supplied
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-5ed5367063e39f890dabafdc476ea2370d32aae7/> remove LatticeTargetId
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Other

 - <csr-id-18dccc362462cc70e82720bb4cb818bda9ae5b12/> v0.12.1
 - <csr-id-e3c96762bda98efeb49bc67605d09453dadaa9ce/> wasmcloud-core v0.11.0
 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC

### Refactor

 - <csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/> re-add missing cache code
 - <csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/> move functionality into core
   This commit moves functionality that was previously located in the
   unreleased `wasmcloud-host` crate into core.
 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/> light refactor from PR followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
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

 - <csr-id-5aebf9bab8b3dfdcb65342c549e8700138ab381f/> Rename OtelProtocol variants to lowercase
 - <csr-id-8676d12373f238286606b17ba7918b308f2144be/> Skip serializing Option fields if set to None
 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/> remove LinkDefinition
 - <csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/> bump to 0.2.0 for async-nats release

### New Features (BREAKING)

 - <csr-id-5f05bcc468b3e67e67a22c666d93176b44164fbc/> add checked set_link_name
 - <csr-id-9045597210b60ea842a91a99d549d58d6440f660/> add hostdata xkeys, secrets as binary
 - <csr-id-3bd9da571cb2a700cbb9a4966d805664a762d9a0/> add trace_level option
 - <csr-id-724b079ef76491e7b030e7db248a2a8364258154/> add secrets to hostdata and links
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

### Bug Fixes (BREAKING)

 - <csr-id-57efd505aebe0f9805221449b7ef3e8557370721/> Fixes issue with provider cache invalidation
   So it turns out that our digest check and invalidation was working
   perfectly, but changes to a provider weren't working properly. This was
   due to the _separate_ caching step for the actual extracted binary. To
   fix this, I made the oci loader return an enum indicating whether it was
   a cache hit or miss and then converted that over to do the right thing
   when loading the par
   
   Also, I bumped the version to 0.14 of wasmcloud-core because that is the
   version that is released on crates.io. I don't know why it ended up that
   way, but I wanted to make sure things reflected reality

### Refactor (BREAKING)

 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 93 commits contributed to the release over the course of 438 calendar days.
 - 88 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Fixes issue with provider cache invalidation ([`57efd50`](https://github.com/wasmCloud/wasmCloud/commit/57efd505aebe0f9805221449b7ef3e8557370721))
    - Bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates ([`c5ba85c`](https://github.com/wasmCloud/wasmCloud/commit/c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33))
    - Improve parse_wit_package_name handling ([`db94b15`](https://github.com/wasmCloud/wasmCloud/commit/db94b15b82f041bd80026c6f3dcf5e5102701e38))
    - V0.12.1 ([`18dccc3`](https://github.com/wasmCloud/wasmCloud/commit/18dccc362462cc70e82720bb4cb818bda9ae5b12))
    - Allow missing registry authentication ([`86863ee`](https://github.com/wasmCloud/wasmCloud/commit/86863ee2ed2e0bb8c2c39844baff5cb8a17119fd))
    - Updates tests and examples to support the new wkg deps ([`f0f3fd7`](https://github.com/wasmCloud/wasmCloud/commit/f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2))
    - Add checked set_link_name ([`5f05bcc`](https://github.com/wasmCloud/wasmCloud/commit/5f05bcc468b3e67e67a22c666d93176b44164fbc))
    - Fix tests, update parsing of advanced WIT package names ([`ea814a1`](https://github.com/wasmCloud/wasmCloud/commit/ea814a1603d9d2ac7173c729024ba2834d97f45b))
    - Bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates ([`44bf4c8`](https://github.com/wasmCloud/wasmCloud/commit/44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a))
    - Revert OtelProtocol rename, add future compatibility aliasing ([`b8d2293`](https://github.com/wasmCloud/wasmCloud/commit/b8d229303bc1f8d1e0983cb5066f7b08bd961bbc))
    - Wasmcloud-core v0.11.0 ([`e3c9676`](https://github.com/wasmCloud/wasmCloud/commit/e3c96762bda98efeb49bc67605d09453dadaa9ce))
    - Rename OtelProtocol variants to lowercase ([`5aebf9b`](https://github.com/wasmCloud/wasmCloud/commit/5aebf9bab8b3dfdcb65342c549e8700138ab381f))
    - Skip serializing Option fields if set to None ([`8676d12`](https://github.com/wasmCloud/wasmCloud/commit/8676d12373f238286606b17ba7918b308f2144be))
    - Use Default impl for Level ([`ebe8ba9`](https://github.com/wasmCloud/wasmCloud/commit/ebe8ba9c7984a158c2c7e787bf02a420be62c530))
    - Remove redundant `tower` dep ([`c205148`](https://github.com/wasmCloud/wasmCloud/commit/c205148b7f67ab5e80edbae46489083fcb665f99))
    - Prep for upgrade to rustls_native_certs v0.8.0 ([`842b1c0`](https://github.com/wasmCloud/wasmCloud/commit/842b1c0f24c78ab5b891be204675748750387424))
    - Bump wasmcloud-core v0.10.0, safety bump 5 crates ([`1af6e05`](https://github.com/wasmCloud/wasmCloud/commit/1af6e05f1a47be4e62a4c21d1704aff2e09bef89))
    - Switch oci-distribution to oci feature ([`d26c69a`](https://github.com/wasmCloud/wasmCloud/commit/d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba))
    - Re-add missing cache code ([`caa9e41`](https://github.com/wasmCloud/wasmCloud/commit/caa9e41b302571c864c56733f3a119da8a2a9a57))
    - Add missing feature for `oci-wasm` ([`21d0601`](https://github.com/wasmCloud/wasmCloud/commit/21d0601b066a29a8b8f182c26372a0adeea290eb))
    - Move functionality into core ([`0547e3a`](https://github.com/wasmCloud/wasmCloud/commit/0547e3a429059b15ec969a0fa36d7823a6b7331f))
    - Bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates ([`8403350`](https://github.com/wasmCloud/wasmCloud/commit/8403350432a2387d4a2bce9c096f002005ba54be))
    - Add RPC subject for provider config updates ([`02d8865`](https://github.com/wasmCloud/wasmCloud/commit/02d88655045d7e620c2452b7d7689cede4ad12db))
    - Bump for test-util release ([`7cd2e71`](https://github.com/wasmCloud/wasmCloud/commit/7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4))
    - Add string and byte utility functions for SecretValue ([`4ffee2e`](https://github.com/wasmCloud/wasmCloud/commit/4ffee2ed95985902071cbdbf8300dba8e2c37d81))
    - Efficiency, pass optional vec secrets ([`cfbf232`](https://github.com/wasmCloud/wasmCloud/commit/cfbf23226f34f3e7245a5d36cd7bb15e1796850c))
    - Impl Zeroize for secret values ([`10e5d70`](https://github.com/wasmCloud/wasmCloud/commit/10e5d702d940a4c36dff542d21c6f56f6c7cb28f))
    - Add hostdata xkeys, secrets as binary ([`9045597`](https://github.com/wasmCloud/wasmCloud/commit/9045597210b60ea842a91a99d549d58d6440f660))
    - Light refactor from PR followup ([`5a6fdbd`](https://github.com/wasmCloud/wasmCloud/commit/5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217))
    - Remove extra trace_level field ([`4e1d6da`](https://github.com/wasmCloud/wasmCloud/commit/4e1d6da189ff49790d876cd244aed89114efba98))
    - Add trace_level option ([`3bd9da5`](https://github.com/wasmCloud/wasmCloud/commit/3bd9da571cb2a700cbb9a4966d805664a762d9a0))
    - Add support for supplying additional CA certificates to OCI and OpenTelemetry clients ([`24e77b7`](https://github.com/wasmCloud/wasmCloud/commit/24e77b7f1f29580ca348a758302cdc6e75cc3afd))
    - Fetch secrets for providers and links ([`e0324d6`](https://github.com/wasmCloud/wasmCloud/commit/e0324d66e49be015b7b231626bc3b619d9374c91))
    - Add secrets to hostdata and links ([`724b079`](https://github.com/wasmCloud/wasmCloud/commit/724b079ef76491e7b030e7db248a2a8364258154))
    - Upgrade `wrpc`, `async-nats`, `wasmtime` ([`9cb1b78`](https://github.com/wasmCloud/wasmCloud/commit/9cb1b784fe7a8892d73bdb40d1172b1879fcd932))
    - Add README and update docs ([`48f6307`](https://github.com/wasmCloud/wasmCloud/commit/48f6307da226a48969d3d08188db89d3d8069495))
    - Bump wascap v0.15.0, wasmcloud-core v0.7.0, wash-lib v0.22.0, wasmcloud-tracing v0.5.0, wasmcloud-provider-sdk v0.6.0, wash-cli v0.29.0, safety bump 5 crates ([`2e38cd4`](https://github.com/wasmCloud/wasmCloud/commit/2e38cd45adef18d47af71b87ca456a25edb2f53a))
    - Add signal-specific path components to OtelConfig's default endpoints ([`56807ae`](https://github.com/wasmCloud/wasmCloud/commit/56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c))
    - Default to http otel protocol if not supplied ([`825ef3a`](https://github.com/wasmCloud/wasmCloud/commit/825ef3a28cbdf49727b902a0a8d5e43aa502c522))
    - Add support for configuring grpc protocol with opentelemetry ([`378b7c8`](https://github.com/wasmCloud/wasmCloud/commit/378b7c89c8b00a5dcee76c06bc8de615dc58f8aa))
    - Configure reqwest with user-agent ([`ac8f773`](https://github.com/wasmCloud/wasmCloud/commit/ac8f773abc171d4083ae5d266c9e9efdf1a0af59))
    - Replace actor references by component in crates ([`20c72ce`](https://github.com/wasmCloud/wasmCloud/commit/20c72ce0ed423561ae6dbd5a91959bec24ff7cf3))
    - Bump provider-archive v0.10.2, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0 ([`73c0ef0`](https://github.com/wasmCloud/wasmCloud/commit/73c0ef0bbe2f6b525655939d2cd30740aef4b6bc))
    - Compile with default features ([`8fc13bf`](https://github.com/wasmCloud/wasmCloud/commit/8fc13bfee8927e9002014ead06762c8a32ed4356))
    - Bump provider-archive v0.10.1, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0, safety bump 5 crates ([`75a2e52`](https://github.com/wasmCloud/wasmCloud/commit/75a2e52f52690ba143679c90237851ebd07e153f))
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

## 0.13.0 (2024-10-23)

<csr-id-db94b15b82f041bd80026c6f3dcf5e5102701e38/>
<csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/>
<csr-id-b8d229303bc1f8d1e0983cb5066f7b08bd961bbc/>
<csr-id-ebe8ba9c7984a158c2c7e787bf02a420be62c530/>
<csr-id-c205148b7f67ab5e80edbae46489083fcb665f99/>
<csr-id-d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
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
<csr-id-18dccc362462cc70e82720bb4cb818bda9ae5b12/>
<csr-id-e3c96762bda98efeb49bc67605d09453dadaa9ce/>
<csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/>
<csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/>
<csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/>
<csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/>
<csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/>
<csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/>
<csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/>
<csr-id-c654448653db224c6a676ecf43150d880a9daf8c/>
<csr-id-c49a6ef0b6460b3eb463315fe31878eb71ae5364/>
<csr-id-123e53611e6d0b2bd4e92358783213784653fbf6/>
<csr-id-7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb/>
<csr-id-0319a9245589709d96b03786374d8026beb5d5d0/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-5aebf9bab8b3dfdcb65342c549e8700138ab381f/>
<csr-id-8676d12373f238286606b17ba7918b308f2144be/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/>
<csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>

### Chore

 - <csr-id-db94b15b82f041bd80026c6f3dcf5e5102701e38/> Improve parse_wit_package_name handling
 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-b8d229303bc1f8d1e0983cb5066f7b08bd961bbc/> Revert OtelProtocol rename, add future compatibility aliasing
 - <csr-id-ebe8ba9c7984a158c2c7e787bf02a420be62c530/> Use Default impl for Level
 - <csr-id-c205148b7f67ab5e80edbae46489083fcb665f99/> remove redundant `tower` dep
 - <csr-id-d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba/> Switch oci-distribution to oci feature
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
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

### Documentation

 - <csr-id-48f6307da226a48969d3d08188db89d3d8069495/> add README and update docs

### New Features

 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-ea814a1603d9d2ac7173c729024ba2834d97f45b/> fix tests, update parsing of advanced WIT package names
 - <csr-id-02d88655045d7e620c2452b7d7689cede4ad12db/> add RPC subject for provider config updates
 - <csr-id-4ffee2ed95985902071cbdbf8300dba8e2c37d81/> add string and byte utility functions for SecretValue
   This commit add some utility functions to enable easily accessing
   string values or byte vector values of `SecretValue`s
 - <csr-id-10e5d702d940a4c36dff542d21c6f56f6c7cb28f/> impl Zeroize for secret values
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/> fetch secrets for providers and links
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
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

### Bug Fixes

 - <csr-id-86863ee2ed2e0bb8c2c39844baff5cb8a17119fd/> allow missing registry authentication
   This commit fixes a bug that ignored the `allow_insecure` setting when
   building registry configuration.
 - <csr-id-842b1c0f24c78ab5b891be204675748750387424/> prep for upgrade to rustls_native_certs v0.8.0
 - <csr-id-21d0601b066a29a8b8f182c26372a0adeea290eb/> add missing feature for `oci-wasm`
 - <csr-id-56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c/> Add signal-specific path components to OtelConfig's default endpoints
 - <csr-id-825ef3a28cbdf49727b902a0a8d5e43aa502c522/> default to http otel protocol if not supplied
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-5ed5367063e39f890dabafdc476ea2370d32aae7/> remove LatticeTargetId
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Other

 - <csr-id-18dccc362462cc70e82720bb4cb818bda9ae5b12/> v0.12.1
 - <csr-id-e3c96762bda98efeb49bc67605d09453dadaa9ce/> wasmcloud-core v0.11.0
 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC

### Refactor

 - <csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/> re-add missing cache code
 - <csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/> move functionality into core
   This commit moves functionality that was previously located in the
   unreleased `wasmcloud-host` crate into core.
 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/> light refactor from PR followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
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

 - <csr-id-5aebf9bab8b3dfdcb65342c549e8700138ab381f/> Rename OtelProtocol variants to lowercase
 - <csr-id-8676d12373f238286606b17ba7918b308f2144be/> Skip serializing Option fields if set to None
 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/> remove LinkDefinition
 - <csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/> bump to 0.2.0 for async-nats release

### New Features (BREAKING)

 - <csr-id-5f05bcc468b3e67e67a22c666d93176b44164fbc/> add checked set_link_name
 - <csr-id-9045597210b60ea842a91a99d549d58d6440f660/> add hostdata xkeys, secrets as binary
 - <csr-id-3bd9da571cb2a700cbb9a4966d805664a762d9a0/> add trace_level option
 - <csr-id-724b079ef76491e7b030e7db248a2a8364258154/> add secrets to hostdata and links
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

## 0.12.1 (2024-10-18)

<csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/>
<csr-id-b8d229303bc1f8d1e0983cb5066f7b08bd961bbc/>
<csr-id-ebe8ba9c7984a158c2c7e787bf02a420be62c530/>
<csr-id-c205148b7f67ab5e80edbae46489083fcb665f99/>
<csr-id-d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
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
<csr-id-e3c96762bda98efeb49bc67605d09453dadaa9ce/>
<csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/>
<csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/>
<csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/>
<csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/>
<csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/>
<csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/>
<csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/>
<csr-id-c654448653db224c6a676ecf43150d880a9daf8c/>
<csr-id-c49a6ef0b6460b3eb463315fe31878eb71ae5364/>
<csr-id-123e53611e6d0b2bd4e92358783213784653fbf6/>
<csr-id-7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb/>
<csr-id-0319a9245589709d96b03786374d8026beb5d5d0/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-5aebf9bab8b3dfdcb65342c549e8700138ab381f/>
<csr-id-8676d12373f238286606b17ba7918b308f2144be/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/>
<csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>

### Chore

 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-b8d229303bc1f8d1e0983cb5066f7b08bd961bbc/> Revert OtelProtocol rename, add future compatibility aliasing
 - <csr-id-ebe8ba9c7984a158c2c7e787bf02a420be62c530/> Use Default impl for Level
 - <csr-id-c205148b7f67ab5e80edbae46489083fcb665f99/> remove redundant `tower` dep
 - <csr-id-d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba/> Switch oci-distribution to oci feature
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
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

### Documentation

 - <csr-id-48f6307da226a48969d3d08188db89d3d8069495/> add README and update docs

### New Features

 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-ea814a1603d9d2ac7173c729024ba2834d97f45b/> fix tests, update parsing of advanced WIT package names
 - <csr-id-02d88655045d7e620c2452b7d7689cede4ad12db/> add RPC subject for provider config updates
 - <csr-id-4ffee2ed95985902071cbdbf8300dba8e2c37d81/> add string and byte utility functions for SecretValue
   This commit add some utility functions to enable easily accessing
   string values or byte vector values of `SecretValue`s
 - <csr-id-10e5d702d940a4c36dff542d21c6f56f6c7cb28f/> impl Zeroize for secret values
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/> fetch secrets for providers and links
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
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

### Bug Fixes

 - <csr-id-86863ee2ed2e0bb8c2c39844baff5cb8a17119fd/> allow missing registry authentication
   This commit fixes a bug that ignored the `allow_insecure` setting when
   building registry configuration.
 - <csr-id-842b1c0f24c78ab5b891be204675748750387424/> prep for upgrade to rustls_native_certs v0.8.0
 - <csr-id-21d0601b066a29a8b8f182c26372a0adeea290eb/> add missing feature for `oci-wasm`
 - <csr-id-56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c/> Add signal-specific path components to OtelConfig's default endpoints
 - <csr-id-825ef3a28cbdf49727b902a0a8d5e43aa502c522/> default to http otel protocol if not supplied
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-5ed5367063e39f890dabafdc476ea2370d32aae7/> remove LatticeTargetId
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Other

 - <csr-id-e3c96762bda98efeb49bc67605d09453dadaa9ce/> wasmcloud-core v0.11.0
 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC

### Refactor

 - <csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/> re-add missing cache code
 - <csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/> move functionality into core
   This commit moves functionality that was previously located in the
   unreleased `wasmcloud-host` crate into core.
 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/> light refactor from PR followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
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

 - <csr-id-5aebf9bab8b3dfdcb65342c549e8700138ab381f/> Rename OtelProtocol variants to lowercase
 - <csr-id-8676d12373f238286606b17ba7918b308f2144be/> Skip serializing Option fields if set to None
 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/> remove LinkDefinition
 - <csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/> bump to 0.2.0 for async-nats release

### New Features (BREAKING)

 - <csr-id-5f05bcc468b3e67e67a22c666d93176b44164fbc/> add checked set_link_name
 - <csr-id-9045597210b60ea842a91a99d549d58d6440f660/> add hostdata xkeys, secrets as binary
 - <csr-id-3bd9da571cb2a700cbb9a4966d805664a762d9a0/> add trace_level option
 - <csr-id-724b079ef76491e7b030e7db248a2a8364258154/> add secrets to hostdata and links
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

## 0.12.0 (2024-10-09)

<csr-id-b8d229303bc1f8d1e0983cb5066f7b08bd961bbc/>
<csr-id-ebe8ba9c7984a158c2c7e787bf02a420be62c530/>
<csr-id-c205148b7f67ab5e80edbae46489083fcb665f99/>
<csr-id-d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
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
<csr-id-e3c96762bda98efeb49bc67605d09453dadaa9ce/>
<csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/>
<csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/>
<csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/>
<csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/>
<csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/>
<csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/>
<csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/>
<csr-id-c654448653db224c6a676ecf43150d880a9daf8c/>
<csr-id-c49a6ef0b6460b3eb463315fe31878eb71ae5364/>
<csr-id-123e53611e6d0b2bd4e92358783213784653fbf6/>
<csr-id-7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb/>
<csr-id-0319a9245589709d96b03786374d8026beb5d5d0/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-5aebf9bab8b3dfdcb65342c549e8700138ab381f/>
<csr-id-8676d12373f238286606b17ba7918b308f2144be/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/>
<csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>

### Chore

 - <csr-id-b8d229303bc1f8d1e0983cb5066f7b08bd961bbc/> Revert OtelProtocol rename, add future compatibility aliasing
 - <csr-id-ebe8ba9c7984a158c2c7e787bf02a420be62c530/> Use Default impl for Level
 - <csr-id-c205148b7f67ab5e80edbae46489083fcb665f99/> remove redundant `tower` dep
 - <csr-id-d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba/> Switch oci-distribution to oci feature
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
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

### Documentation

 - <csr-id-48f6307da226a48969d3d08188db89d3d8069495/> add README and update docs

### New Features

 - <csr-id-02d88655045d7e620c2452b7d7689cede4ad12db/> add RPC subject for provider config updates
 - <csr-id-4ffee2ed95985902071cbdbf8300dba8e2c37d81/> add string and byte utility functions for SecretValue
   This commit add some utility functions to enable easily accessing
   string values or byte vector values of `SecretValue`s
 - <csr-id-10e5d702d940a4c36dff542d21c6f56f6c7cb28f/> impl Zeroize for secret values
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/> fetch secrets for providers and links
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
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

### Bug Fixes

 - <csr-id-842b1c0f24c78ab5b891be204675748750387424/> prep for upgrade to rustls_native_certs v0.8.0
 - <csr-id-21d0601b066a29a8b8f182c26372a0adeea290eb/> add missing feature for `oci-wasm`
 - <csr-id-56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c/> Add signal-specific path components to OtelConfig's default endpoints
 - <csr-id-825ef3a28cbdf49727b902a0a8d5e43aa502c522/> default to http otel protocol if not supplied
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-5ed5367063e39f890dabafdc476ea2370d32aae7/> remove LatticeTargetId
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Other

 - <csr-id-e3c96762bda98efeb49bc67605d09453dadaa9ce/> wasmcloud-core v0.11.0
 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC

### Refactor

 - <csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/> re-add missing cache code
 - <csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/> move functionality into core
   This commit moves functionality that was previously located in the
   unreleased `wasmcloud-host` crate into core.
 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/> light refactor from PR followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
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

 - <csr-id-5aebf9bab8b3dfdcb65342c549e8700138ab381f/> Rename OtelProtocol variants to lowercase
 - <csr-id-8676d12373f238286606b17ba7918b308f2144be/> Skip serializing Option fields if set to None
 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/> remove LinkDefinition
 - <csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/> bump to 0.2.0 for async-nats release

### New Features (BREAKING)

 - <csr-id-9045597210b60ea842a91a99d549d58d6440f660/> add hostdata xkeys, secrets as binary
 - <csr-id-3bd9da571cb2a700cbb9a4966d805664a762d9a0/> add trace_level option
 - <csr-id-724b079ef76491e7b030e7db248a2a8364258154/> add secrets to hostdata and links
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

## 0.11.0 (2024-09-30)

<csr-id-ebe8ba9c7984a158c2c7e787bf02a420be62c530/>
<csr-id-c205148b7f67ab5e80edbae46489083fcb665f99/>
<csr-id-d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
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
<csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/>
<csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/>
<csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/>
<csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/>
<csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-87eb6c8b2c0bd31def1cfdc6121c612c4dc90871/>
<csr-id-5d7383137897d28a1bc5df9b1c48f75281dab55b/>
<csr-id-b6a6b04229730d6783c3fee61c6e078cd3b962ef/>
<csr-id-c654448653db224c6a676ecf43150d880a9daf8c/>
<csr-id-c49a6ef0b6460b3eb463315fe31878eb71ae5364/>
<csr-id-123e53611e6d0b2bd4e92358783213784653fbf6/>
<csr-id-7402a1f5cc4515e270fa66bbdd3d8bf2c03f35cb/>
<csr-id-0319a9245589709d96b03786374d8026beb5d5d0/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-5aebf9bab8b3dfdcb65342c549e8700138ab381f/>
<csr-id-8676d12373f238286606b17ba7918b308f2144be/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/>
<csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>

### Chore

 - <csr-id-ebe8ba9c7984a158c2c7e787bf02a420be62c530/> Use Default impl for Level
 - <csr-id-c205148b7f67ab5e80edbae46489083fcb665f99/> remove redundant `tower` dep
 - <csr-id-d26c69a22749bc92b8bfd2f4c93d0c9d3cc744ba/> Switch oci-distribution to oci feature
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
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

### Documentation

 - <csr-id-48f6307da226a48969d3d08188db89d3d8069495/> add README and update docs

### New Features

 - <csr-id-02d88655045d7e620c2452b7d7689cede4ad12db/> add RPC subject for provider config updates
 - <csr-id-4ffee2ed95985902071cbdbf8300dba8e2c37d81/> add string and byte utility functions for SecretValue
   This commit add some utility functions to enable easily accessing
   string values or byte vector values of `SecretValue`s
 - <csr-id-10e5d702d940a4c36dff542d21c6f56f6c7cb28f/> impl Zeroize for secret values
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/> fetch secrets for providers and links
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
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

### Bug Fixes

 - <csr-id-842b1c0f24c78ab5b891be204675748750387424/> prep for upgrade to rustls_native_certs v0.8.0
 - <csr-id-21d0601b066a29a8b8f182c26372a0adeea290eb/> add missing feature for `oci-wasm`
 - <csr-id-56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c/> Add signal-specific path components to OtelConfig's default endpoints
 - <csr-id-825ef3a28cbdf49727b902a0a8d5e43aa502c522/> default to http otel protocol if not supplied
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-5ed5367063e39f890dabafdc476ea2370d32aae7/> remove LatticeTargetId
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Other

 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC

### Refactor

 - <csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/> re-add missing cache code
 - <csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/> move functionality into core
   This commit moves functionality that was previously located in the
   unreleased `wasmcloud-host` crate into core.
 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/> light refactor from PR followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
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

 - <csr-id-5aebf9bab8b3dfdcb65342c549e8700138ab381f/> Rename OtelProtocol variants to lowercase
 - <csr-id-8676d12373f238286606b17ba7918b308f2144be/> Skip serializing Option fields if set to None
 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-8e7d6c80b56e143bb09dc441e8b21104328d0ab0/> remove LinkDefinition
 - <csr-id-6abbcac954a9834d871ea69b8a40bd79d258c0f1/> bump to 0.2.0 for async-nats release

### New Features (BREAKING)

 - <csr-id-9045597210b60ea842a91a99d549d58d6440f660/> add hostdata xkeys, secrets as binary
 - <csr-id-3bd9da571cb2a700cbb9a4966d805664a762d9a0/> add trace_level option
 - <csr-id-724b079ef76491e7b030e7db248a2a8364258154/> add secrets to hostdata and links
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

## 0.10.0 (2024-09-18)

<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
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
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/>
<csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/>
<csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/>
<csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/>
<csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
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

### Chore

 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
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

### Documentation

 - <csr-id-48f6307da226a48969d3d08188db89d3d8069495/> add README and update docs

### New Features

 - <csr-id-02d88655045d7e620c2452b7d7689cede4ad12db/> add RPC subject for provider config updates
 - <csr-id-4ffee2ed95985902071cbdbf8300dba8e2c37d81/> add string and byte utility functions for SecretValue
   This commit add some utility functions to enable easily accessing
   string values or byte vector values of `SecretValue`s
 - <csr-id-10e5d702d940a4c36dff542d21c6f56f6c7cb28f/> impl Zeroize for secret values
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/> fetch secrets for providers and links
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
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

### Bug Fixes

 - <csr-id-21d0601b066a29a8b8f182c26372a0adeea290eb/> add missing feature for `oci-wasm`
 - <csr-id-56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c/> Add signal-specific path components to OtelConfig's default endpoints
 - <csr-id-825ef3a28cbdf49727b902a0a8d5e43aa502c522/> default to http otel protocol if not supplied
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-5ed5367063e39f890dabafdc476ea2370d32aae7/> remove LatticeTargetId
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Other

 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC

### Refactor

 - <csr-id-caa9e41b302571c864c56733f3a119da8a2a9a57/> re-add missing cache code
 - <csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/> move functionality into core
   This commit moves functionality that was previously located in the
   unreleased `wasmcloud-host` crate into core.
 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/> light refactor from PR followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
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

 - <csr-id-9045597210b60ea842a91a99d549d58d6440f660/> add hostdata xkeys, secrets as binary
 - <csr-id-3bd9da571cb2a700cbb9a4966d805664a762d9a0/> add trace_level option
 - <csr-id-724b079ef76491e7b030e7db248a2a8364258154/> add secrets to hostdata and links
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

## 0.9.0 (2024-08-23)

<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
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
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/>
<csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/>
<csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
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

### Chore

 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
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

### Documentation

 - <csr-id-48f6307da226a48969d3d08188db89d3d8069495/> add README and update docs

### New Features

 - <csr-id-02d88655045d7e620c2452b7d7689cede4ad12db/> add RPC subject for provider config updates
 - <csr-id-4ffee2ed95985902071cbdbf8300dba8e2c37d81/> add string and byte utility functions for SecretValue
   This commit add some utility functions to enable easily accessing
   string values or byte vector values of `SecretValue`s
 - <csr-id-10e5d702d940a4c36dff542d21c6f56f6c7cb28f/> impl Zeroize for secret values
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/> fetch secrets for providers and links
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
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

### Bug Fixes

 - <csr-id-56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c/> Add signal-specific path components to OtelConfig's default endpoints
 - <csr-id-825ef3a28cbdf49727b902a0a8d5e43aa502c522/> default to http otel protocol if not supplied
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-5ed5367063e39f890dabafdc476ea2370d32aae7/> remove LatticeTargetId
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Other

 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC

### Refactor

 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/> light refactor from PR followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
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

 - <csr-id-9045597210b60ea842a91a99d549d58d6440f660/> add hostdata xkeys, secrets as binary
 - <csr-id-3bd9da571cb2a700cbb9a4966d805664a762d9a0/> add trace_level option
 - <csr-id-724b079ef76491e7b030e7db248a2a8364258154/> add secrets to hostdata and links
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

## 0.8.0 (2024-07-31)

<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
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
<csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/>
<csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
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

### Chore

 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
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

### Documentation

 - <csr-id-48f6307da226a48969d3d08188db89d3d8069495/> add README and update docs

### New Features

 - <csr-id-4ffee2ed95985902071cbdbf8300dba8e2c37d81/> add string and byte utility functions for SecretValue
   This commit add some utility functions to enable easily accessing
   string values or byte vector values of `SecretValue`s
 - <csr-id-10e5d702d940a4c36dff542d21c6f56f6c7cb28f/> impl Zeroize for secret values
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/> fetch secrets for providers and links
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
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

### Bug Fixes

 - <csr-id-56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c/> Add signal-specific path components to OtelConfig's default endpoints
 - <csr-id-825ef3a28cbdf49727b902a0a8d5e43aa502c522/> default to http otel protocol if not supplied
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-5ed5367063e39f890dabafdc476ea2370d32aae7/> remove LatticeTargetId
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-a896f05a35824f5e2ba16fdb1c1f5217c52a5388/> enable `std` anyhow feature

### Other

 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC

### Refactor

 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-5a6fdbda50d91f23c3fc6ea2b28dfe55edd46217/> light refactor from PR followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
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

 - <csr-id-9045597210b60ea842a91a99d549d58d6440f660/> add hostdata xkeys, secrets as binary
 - <csr-id-3bd9da571cb2a700cbb9a4966d805664a762d9a0/> add trace_level option
 - <csr-id-724b079ef76491e7b030e7db248a2a8364258154/> add secrets to hostdata and links
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

## 0.7.0 (2024-06-11)

<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
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

### Chore

 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
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

### New Features

 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
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

### Bug Fixes

 - <csr-id-56807ae5d0f6bbddb12f0e22d58a3d84fdb4f48c/> Add signal-specific path components to OtelConfig's default endpoints
 - <csr-id-825ef3a28cbdf49727b902a0a8d5e43aa502c522/> default to http otel protocol if not supplied
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features
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
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>

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
 - <csr-id-8fc13bfee8927e9002014ead06762c8a32ed4356/> compile with default features

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


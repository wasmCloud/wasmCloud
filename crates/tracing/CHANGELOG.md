# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.11.0 (2024-11-08)

### Chore

 - <csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/> bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, wasmcloud-host v0.22.0, wasmcloud-runtime v0.6.0, wasmcloud-test-util v0.14.0
 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/> gate import behind feature
 - <csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/> Bump opentelemetry-* crates and tracing-opentelemetry to latest version
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/> bump to 0.3.0
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-53a312c3c35014e1b337a45a96373b81512bc113/> bump to 0.2.0
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/> Update the default OTLP HTTP port to match the current spec
 - <csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/> Remove check for default OpenTelemetry traces path
 - <csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/> Normalize service.names to use kebab-case
 - <csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/> address clippy warnings
 - <csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/> clean-up imports
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting

### New Features

 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e28361935ad3b09d46658488e813c809522317bf/> add support for flame graphs
 - <csr-id-1b076b3479874dbc2f7e575fcee65bab66bd056d/> use `tracing-appender`
   Avoid locking whole process on each logging statement
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
   * the count of errors (`wasmcloud_host.actor.invocation.errors`)
   
   This also adds a bunch of new attributes to the existing actor metrics so that they make sense in an environment with multiple hosts. Specifically this adds:
   * the lattice ID
   * the host ID
   * provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef
   
   For actor to actor calls, instead of having the provider metadata it instead has the public key of the invoking actor.
   
   An example of what this looks like as an exported Prometheus metric:
   
   ```
   wasmcloud_host_actor_invocations_total{actor_ref="wasmcloud.azurecr.io/echo:0.3.8", caller_provider_contract_id="wasmcloud:httpserver", caller_provider_id="VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", caller_provider_link_name="default", host="ND7L3RZ6NLYJGN25E6DKYS665ITWXAPXZXGZXLCUQEDDU65RB5TVUHEN", job="wasmcloud-host", lattice="default", operation="HttpServer.HandleRequest"}
   ```
   
   Provider metrics will likely need to wait until the wRPC work is finished.
 - <csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/> Add initial support for metrics
 - <csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/> enable OTEL logs
 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end

### Bug Fixes

 - <csr-id-3d2cce2e3dcbd23bb7d29c3a4c126650df889b0d/> enable core otel features
 - <csr-id-8b66966e03e5d9c2fa64fb2480e9264f3f9e2f55/> include rustls-tls feature for reqwest
 - <csr-id-a10e171e16e08f16e21ad07fff99343b10363fc9/> unused metrics functions
 - <csr-id-f38f5510fc53ea83a94378851a02c3800444388f/> fix compilation issue in tracing
 - <csr-id-8d345114fbd30a3f6784d2b22fa79f1c44f807c5/> split directives before trying to parse
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-46b441d1358fd0ee349bf1dfc87236c400cb4db1/> reduce verbosity of nats logs
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/> pub the context mod only with the otel feature enabled

### Other

 - <csr-id-1ff476dcd61675a81d747091a1a94f1a4cd5fedb/> tracing v0.8.0, provider-sdk v0.9.0, wash-cli v0.33.0
 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0

### Refactor

 - <csr-id-517c6add84b6ab6062b5a0bc2511e5585f9ad3db/> fix lint
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-3048567a115618599336d4c1255fc6324c04a203/> simplify registry construction
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers

### Style

 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports

### New Features (BREAKING)

 - <csr-id-8ad2cde49cb52872af4c9753be7c422092ae56ee/> add trace_level option
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Bug Fixes (BREAKING)

 - <csr-id-ca8959ab707196cb061b7b732c4ed9e3a92e2c88/> return Dispatch from configure_tracing()
   This commit returns Dispatch from `configure_tracing()` rather than
   attempting to call `set_global_default()`[0].
   
   `set_global_default` cannot safely be called from libraries
   that are used in processes that will be executing (the host and a
   binar provider, in the wasmCloud case).
   
   Setting the default must be done from closer to the binar -- at the
   very least not in a shared library that both the host and provider use.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 56 commits contributed to the release over the course of 438 calendar days.
 - 51 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates ([`c5ba85c`](https://github.com/wasmCloud/wasmCloud/commit/c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33))
    - Updates tests and examples to support the new wkg deps ([`f0f3fd7`](https://github.com/wasmCloud/wasmCloud/commit/f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2))
    - Bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates ([`44bf4c8`](https://github.com/wasmCloud/wasmCloud/commit/44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a))
    - Tracing v0.8.0, provider-sdk v0.9.0, wash-cli v0.33.0 ([`1ff476d`](https://github.com/wasmCloud/wasmCloud/commit/1ff476dcd61675a81d747091a1a94f1a4cd5fedb))
    - Bump wasmcloud-core v0.10.0, safety bump 5 crates ([`1af6e05`](https://github.com/wasmCloud/wasmCloud/commit/1af6e05f1a47be4e62a4c21d1704aff2e09bef89))
    - Fix lint ([`517c6ad`](https://github.com/wasmCloud/wasmCloud/commit/517c6add84b6ab6062b5a0bc2511e5585f9ad3db))
    - Bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates ([`8403350`](https://github.com/wasmCloud/wasmCloud/commit/8403350432a2387d4a2bce9c096f002005ba54be))
    - Enable core otel features ([`3d2cce2`](https://github.com/wasmCloud/wasmCloud/commit/3d2cce2e3dcbd23bb7d29c3a4c126650df889b0d))
    - Bump for test-util release ([`7cd2e71`](https://github.com/wasmCloud/wasmCloud/commit/7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4))
    - Include rustls-tls feature for reqwest ([`8b66966`](https://github.com/wasmCloud/wasmCloud/commit/8b66966e03e5d9c2fa64fb2480e9264f3f9e2f55))
    - Remove extra trace_level field ([`4e1d6da`](https://github.com/wasmCloud/wasmCloud/commit/4e1d6da189ff49790d876cd244aed89114efba98))
    - Add trace_level option ([`8ad2cde`](https://github.com/wasmCloud/wasmCloud/commit/8ad2cde49cb52872af4c9753be7c422092ae56ee))
    - Add support for supplying additional CA certificates to OCI and OpenTelemetry clients ([`24e77b7`](https://github.com/wasmCloud/wasmCloud/commit/24e77b7f1f29580ca348a758302cdc6e75cc3afd))
    - Return Dispatch from configure_tracing() ([`ca8959a`](https://github.com/wasmCloud/wasmCloud/commit/ca8959ab707196cb061b7b732c4ed9e3a92e2c88))
    - Add support for flame graphs ([`e283619`](https://github.com/wasmCloud/wasmCloud/commit/e28361935ad3b09d46658488e813c809522317bf))
    - Simplify registry construction ([`3048567`](https://github.com/wasmCloud/wasmCloud/commit/3048567a115618599336d4c1255fc6324c04a203))
    - Use `tracing-appender` ([`1b076b3`](https://github.com/wasmCloud/wasmCloud/commit/1b076b3479874dbc2f7e575fcee65bab66bd056d))
    - Bump wasmcloud-tracing v0.5.0, wasmcloud-provider-sdk v0.6.0, wash-cli v0.29.0 ([`b22d338`](https://github.com/wasmCloud/wasmCloud/commit/b22d338d0d61f8a438c4d6ea5e8e5cd26116ade5))
    - Gate import behind feature ([`c262023`](https://github.com/wasmCloud/wasmCloud/commit/c262023ea20c256686d7f1bdd1d6b21b031b55a6))
    - Bump wascap v0.15.0, wasmcloud-core v0.7.0, wash-lib v0.22.0, wasmcloud-tracing v0.5.0, wasmcloud-provider-sdk v0.6.0, wash-cli v0.29.0, safety bump 5 crates ([`2e38cd4`](https://github.com/wasmCloud/wasmCloud/commit/2e38cd45adef18d47af71b87ca456a25edb2f53a))
    - Add support for configuring grpc protocol with opentelemetry ([`378b7c8`](https://github.com/wasmCloud/wasmCloud/commit/378b7c89c8b00a5dcee76c06bc8de615dc58f8aa))
    - Bump opentelemetry-* crates and tracing-opentelemetry to latest version ([`44d35f2`](https://github.com/wasmCloud/wasmCloud/commit/44d35f268e1c55a1fbb91f2bc27b43a19c4581fe))
    - Bump provider-archive v0.10.2, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0 ([`73c0ef0`](https://github.com/wasmCloud/wasmCloud/commit/73c0ef0bbe2f6b525655939d2cd30740aef4b6bc))
    - Bump provider-archive v0.10.1, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0, safety bump 5 crates ([`75a2e52`](https://github.com/wasmCloud/wasmCloud/commit/75a2e52f52690ba143679c90237851ebd07e153f))
    - Generate changelogs after 1.0.1 release ([`4e0313a`](https://github.com/wasmCloud/wasmCloud/commit/4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e))
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - Bump to 0.3.0 ([`be57edb`](https://github.com/wasmCloud/wasmCloud/commit/be57edb70fe783ca71c2eadc7f27d68e5712b3e7))
    - Excises vestigal remains of wasmbus-rpc ([`fd69df4`](https://github.com/wasmCloud/wasmCloud/commit/fd69df40f24ca565ace0f8c97a0c47a89db575a4))
    - Unused metrics functions ([`a10e171`](https://github.com/wasmCloud/wasmCloud/commit/a10e171e16e08f16e21ad07fff99343b10363fc9))
    - Fix compilation issue in tracing ([`f38f551`](https://github.com/wasmCloud/wasmCloud/commit/f38f5510fc53ea83a94378851a02c3800444388f))
    - Bump to 0.2.0 ([`53a312c`](https://github.com/wasmCloud/wasmCloud/commit/53a312c3c35014e1b337a45a96373b81512bc113))
    - Use traces instead of tracing user-facing language to align with OTEL signal names ([`d65512b`](https://github.com/wasmCloud/wasmCloud/commit/d65512b5e86eb4d13e64cffa220a5a842c7bb72b))
    - Add flags for overriding the default OpenTelemetry endpoint ([`6fe14b8`](https://github.com/wasmCloud/wasmCloud/commit/6fe14b89d4c26e5c01e54773268c6d0f04236e71))
    - Switch to using --enable-observability and --enable-<signal> flags ([`868570b`](https://github.com/wasmCloud/wasmCloud/commit/868570be8d94a6d73608c7cde5d2422e15f9eb0c))
    - Add invocation and error counts for actor invocations ([`7d51408`](https://github.com/wasmCloud/wasmCloud/commit/7d51408440509c687b01e00b77a3672a8e8c30c9))
    - Updates topics to the new standard ([`42d069e`](https://github.com/wasmCloud/wasmCloud/commit/42d069eee87d1b5befff1a95b49973064f1a1d1b))
    - Add initial support for metrics ([`17648fe`](https://github.com/wasmCloud/wasmCloud/commit/17648fedc2a1907b2f0c6d053b9747e72999addb))
    - Enable OTEL logs ([`3602bdf`](https://github.com/wasmCloud/wasmCloud/commit/3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3))
    - Update the default OTLP HTTP port to match the current spec ([`cdf389b`](https://github.com/wasmCloud/wasmCloud/commit/cdf389bdda44fbccfb0f513d84f3737722f0a1a7))
    - Remove check for default OpenTelemetry traces path ([`71f8bc0`](https://github.com/wasmCloud/wasmCloud/commit/71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d))
    - Normalize service.names to use kebab-case ([`2b52f08`](https://github.com/wasmCloud/wasmCloud/commit/2b52f083fde88b98a20dd53ba24e4ae697fcef16))
    - Address clippy warnings ([`fffc9bb`](https://github.com/wasmCloud/wasmCloud/commit/fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31))
    - Clean-up imports ([`45eea2a`](https://github.com/wasmCloud/wasmCloud/commit/45eea2ae0f65a0f4f403bed14feefdd67f82d0f3))
    - Add descriptions to crates ([`cb0bcab`](https://github.com/wasmCloud/wasmCloud/commit/cb0bcab822cb4290c673051ec1dd98d034a61546))
    - Split directives before trying to parse ([`8d34511`](https://github.com/wasmCloud/wasmCloud/commit/8d345114fbd30a3f6784d2b22fa79f1c44f807c5))
    - Proxy RUST_LOG to providers ([`691c371`](https://github.com/wasmCloud/wasmCloud/commit/691c3719b8030e437f565156ad5b9cff12fd4cf3))
    - Satisfy clippy linting ([`1a80eea`](https://github.com/wasmCloud/wasmCloud/commit/1a80eeaa1f1ba333891092f8a27e924511c0bd68))
    - Reduce verbosity of nats logs ([`46b441d`](https://github.com/wasmCloud/wasmCloud/commit/46b441d1358fd0ee349bf1dfc87236c400cb4db1))
    - Filter verbose logs ([`5ead09f`](https://github.com/wasmCloud/wasmCloud/commit/5ead09f6ee292e4923dcbfcce64ee3d6081dca2d))
    - Attach traces on inbound and outbound messages ([`74142c4`](https://github.com/wasmCloud/wasmCloud/commit/74142c4cff683565fb321b7b65fbb158b5a9c990))
    - Pub the context mod only with the otel feature enabled ([`45b0fb0`](https://github.com/wasmCloud/wasmCloud/commit/45b0fb0960921a4eebd335977fd8bc747def97a4))
    - Replace lazy_static with once_cell ([`e1d7356`](https://github.com/wasmCloud/wasmCloud/commit/e1d7356bb0a07af9f4e6b1626f5df33709f3ed78))
    - Update imports ([`a8538fb`](https://github.com/wasmCloud/wasmCloud/commit/a8538fb7926b190a180bdd2b46ad00757d98759a))
    - Construct a strongly typed HostData to send to providers ([`23f1759`](https://github.com/wasmCloud/wasmCloud/commit/23f1759e818117f007df8d9b1bdfdfa7710c98c5))
    - Support OTEL traces end-to-end ([`675d364`](https://github.com/wasmCloud/wasmCloud/commit/675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6))
</details>

## 0.10.0 (2024-10-23)

<csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/>
<csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/>
<csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-53a312c3c35014e1b337a45a96373b81512bc113/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/>
<csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/>
<csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/>
<csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/>
<csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-1ff476dcd61675a81d747091a1a94f1a4cd5fedb/>
<csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-517c6add84b6ab6062b5a0bc2511e5585f9ad3db/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-3048567a115618599336d4c1255fc6324c04a203/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>

### Chore

 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/> gate import behind feature
 - <csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/> Bump opentelemetry-* crates and tracing-opentelemetry to latest version
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/> bump to 0.3.0
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-53a312c3c35014e1b337a45a96373b81512bc113/> bump to 0.2.0
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/> Update the default OTLP HTTP port to match the current spec
 - <csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/> Remove check for default OpenTelemetry traces path
 - <csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/> Normalize service.names to use kebab-case
 - <csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/> address clippy warnings
 - <csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/> clean-up imports
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting

### New Features

<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>
<csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/>
<csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/>

 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e28361935ad3b09d46658488e813c809522317bf/> add support for flame graphs
 - <csr-id-1b076b3479874dbc2f7e575fcee65bab66bd056d/> use `tracing-appender`
   Avoid locking whole process on each logging statement
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef

### Bug Fixes

 - <csr-id-3d2cce2e3dcbd23bb7d29c3a4c126650df889b0d/> enable core otel features
 - <csr-id-8b66966e03e5d9c2fa64fb2480e9264f3f9e2f55/> include rustls-tls feature for reqwest
 - <csr-id-a10e171e16e08f16e21ad07fff99343b10363fc9/> unused metrics functions
 - <csr-id-f38f5510fc53ea83a94378851a02c3800444388f/> fix compilation issue in tracing
 - <csr-id-8d345114fbd30a3f6784d2b22fa79f1c44f807c5/> split directives before trying to parse
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-46b441d1358fd0ee349bf1dfc87236c400cb4db1/> reduce verbosity of nats logs
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/> pub the context mod only with the otel feature enabled

### Other

 - <csr-id-1ff476dcd61675a81d747091a1a94f1a4cd5fedb/> tracing v0.8.0, provider-sdk v0.9.0, wash-cli v0.33.0
 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0

### Refactor

 - <csr-id-517c6add84b6ab6062b5a0bc2511e5585f9ad3db/> fix lint
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-3048567a115618599336d4c1255fc6324c04a203/> simplify registry construction
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers

### Style

 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports

### New Features (BREAKING)

 - <csr-id-8ad2cde49cb52872af4c9753be7c422092ae56ee/> add trace_level option
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Bug Fixes (BREAKING)

 - <csr-id-ca8959ab707196cb061b7b732c4ed9e3a92e2c88/> return Dispatch from configure_tracing()
   This commit returns Dispatch from `configure_tracing()` rather than
   attempting to call `set_global_default()`[0].
   
   `set_global_default` cannot safely be called from libraries
   that are used in processes that will be executing (the host and a
   binar provider, in the wasmCloud case).
   
   Setting the default must be done from closer to the binar -- at the
   very least not in a shared library that both the host and provider use.

<csr-unknown>
This also adds a bunch of new attributes to the existing actor metrics so that they make sense in an environment with multiple hosts. Specifically this adds:For actor to actor calls, instead of having the provider metadata it instead has the public key of the invoking actor.An example of what this looks like as an exported Prometheus metric:wasmcloud_host_actor_invocations_total{actor_ref="wasmcloud.azurecr.io/echo:0.3.8", caller_provider_contract_id="wasmcloud:httpserver", caller_provider_id="VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", caller_provider_link_name="default", host="ND7L3RZ6NLYJGN25E6DKYS665ITWXAPXZXGZXLCUQEDDU65RB5TVUHEN", job="wasmcloud-host", lattice="default", operation="HttpServer.HandleRequest"}
Provider metrics will likely need to wait until the wRPC work is finished. Add initial support for metrics enable OTEL logs support OTEL traces end-to-end<csr-unknown/>

## 0.9.0 (2024-10-09)

<csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/>
<csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-53a312c3c35014e1b337a45a96373b81512bc113/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/>
<csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/>
<csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/>
<csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/>
<csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-1ff476dcd61675a81d747091a1a94f1a4cd5fedb/>
<csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-517c6add84b6ab6062b5a0bc2511e5585f9ad3db/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-3048567a115618599336d4c1255fc6324c04a203/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>

### Chore

 - <csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/> gate import behind feature
 - <csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/> Bump opentelemetry-* crates and tracing-opentelemetry to latest version
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/> bump to 0.3.0
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-53a312c3c35014e1b337a45a96373b81512bc113/> bump to 0.2.0
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/> Update the default OTLP HTTP port to match the current spec
 - <csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/> Remove check for default OpenTelemetry traces path
 - <csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/> Normalize service.names to use kebab-case
 - <csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/> address clippy warnings
 - <csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/> clean-up imports
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting

### New Features

<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>
<csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/>
<csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/>

 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e28361935ad3b09d46658488e813c809522317bf/> add support for flame graphs
 - <csr-id-1b076b3479874dbc2f7e575fcee65bab66bd056d/> use `tracing-appender`
   Avoid locking whole process on each logging statement
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef

### Bug Fixes

 - <csr-id-3d2cce2e3dcbd23bb7d29c3a4c126650df889b0d/> enable core otel features
 - <csr-id-8b66966e03e5d9c2fa64fb2480e9264f3f9e2f55/> include rustls-tls feature for reqwest
 - <csr-id-a10e171e16e08f16e21ad07fff99343b10363fc9/> unused metrics functions
 - <csr-id-f38f5510fc53ea83a94378851a02c3800444388f/> fix compilation issue in tracing
 - <csr-id-8d345114fbd30a3f6784d2b22fa79f1c44f807c5/> split directives before trying to parse
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-46b441d1358fd0ee349bf1dfc87236c400cb4db1/> reduce verbosity of nats logs
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/> pub the context mod only with the otel feature enabled

### Other

 - <csr-id-1ff476dcd61675a81d747091a1a94f1a4cd5fedb/> tracing v0.8.0, provider-sdk v0.9.0, wash-cli v0.33.0
 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0

### Refactor

 - <csr-id-517c6add84b6ab6062b5a0bc2511e5585f9ad3db/> fix lint
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-3048567a115618599336d4c1255fc6324c04a203/> simplify registry construction
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers

### Style

 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports

### New Features (BREAKING)

 - <csr-id-8ad2cde49cb52872af4c9753be7c422092ae56ee/> add trace_level option
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Bug Fixes (BREAKING)

 - <csr-id-ca8959ab707196cb061b7b732c4ed9e3a92e2c88/> return Dispatch from configure_tracing()
   This commit returns Dispatch from `configure_tracing()` rather than
   attempting to call `set_global_default()`[0].
   
   `set_global_default` cannot safely be called from libraries
   that are used in processes that will be executing (the host and a
   binar provider, in the wasmCloud case).
   
   Setting the default must be done from closer to the binar -- at the
   very least not in a shared library that both the host and provider use.

<csr-unknown>
<csr-unknown>
This also adds a bunch of new attributes to the existing actor metrics so that they make sense in an environment with multiple hosts. Specifically this adds:For actor to actor calls, instead of having the provider metadata it instead has the public key of the invoking actor.An example of what this looks like as an exported Prometheus metric:wasmcloud_host_actor_invocations_total{actor_ref="wasmcloud.azurecr.io/echo:0.3.8", caller_provider_contract_id="wasmcloud:httpserver", caller_provider_id="VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", caller_provider_link_name="default", host="ND7L3RZ6NLYJGN25E6DKYS665ITWXAPXZXGZXLCUQEDDU65RB5TVUHEN", job="wasmcloud-host", lattice="default", operation="HttpServer.HandleRequest"}
Provider metrics will likely need to wait until the wRPC work is finished. Add initial support for metrics enable OTEL logs support OTEL traces end-to-end<csr-unknown/>
<csr-unknown/>

## 0.8.0 (2024-09-18)

<csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/>
<csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-53a312c3c35014e1b337a45a96373b81512bc113/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/>
<csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/>
<csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/>
<csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/>
<csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-517c6add84b6ab6062b5a0bc2511e5585f9ad3db/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-3048567a115618599336d4c1255fc6324c04a203/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>

### Chore

 - <csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/> gate import behind feature
 - <csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/> Bump opentelemetry-* crates and tracing-opentelemetry to latest version
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/> bump to 0.3.0
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-53a312c3c35014e1b337a45a96373b81512bc113/> bump to 0.2.0
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/> Update the default OTLP HTTP port to match the current spec
 - <csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/> Remove check for default OpenTelemetry traces path
 - <csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/> Normalize service.names to use kebab-case
 - <csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/> address clippy warnings
 - <csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/> clean-up imports
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting

### New Features

<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>
<csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/>
<csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/>

 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e28361935ad3b09d46658488e813c809522317bf/> add support for flame graphs
 - <csr-id-1b076b3479874dbc2f7e575fcee65bab66bd056d/> use `tracing-appender`
   Avoid locking whole process on each logging statement
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef

### Bug Fixes

 - <csr-id-3d2cce2e3dcbd23bb7d29c3a4c126650df889b0d/> enable core otel features
 - <csr-id-8b66966e03e5d9c2fa64fb2480e9264f3f9e2f55/> include rustls-tls feature for reqwest
 - <csr-id-a10e171e16e08f16e21ad07fff99343b10363fc9/> unused metrics functions
 - <csr-id-f38f5510fc53ea83a94378851a02c3800444388f/> fix compilation issue in tracing
 - <csr-id-8d345114fbd30a3f6784d2b22fa79f1c44f807c5/> split directives before trying to parse
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-46b441d1358fd0ee349bf1dfc87236c400cb4db1/> reduce verbosity of nats logs
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/> pub the context mod only with the otel feature enabled

### Other

 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0

### Refactor

 - <csr-id-517c6add84b6ab6062b5a0bc2511e5585f9ad3db/> fix lint
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-3048567a115618599336d4c1255fc6324c04a203/> simplify registry construction
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers

### Style

 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports

### New Features (BREAKING)

 - <csr-id-8ad2cde49cb52872af4c9753be7c422092ae56ee/> add trace_level option
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Bug Fixes (BREAKING)

 - <csr-id-ca8959ab707196cb061b7b732c4ed9e3a92e2c88/> return Dispatch from configure_tracing()
   This commit returns Dispatch from `configure_tracing()` rather than
   attempting to call `set_global_default()`[0].
   
   `set_global_default` cannot safely be called from libraries
   that are used in processes that will be executing (the host and a
   binar provider, in the wasmCloud case).
   
   Setting the default must be done from closer to the binar -- at the
   very least not in a shared library that both the host and provider use.

<csr-unknown>
<csr-unknown>
<csr-unknown>
This also adds a bunch of new attributes to the existing actor metrics so that they make sense in an environment with multiple hosts. Specifically this adds:For actor to actor calls, instead of having the provider metadata it instead has the public key of the invoking actor.An example of what this looks like as an exported Prometheus metric:wasmcloud_host_actor_invocations_total{actor_ref="wasmcloud.azurecr.io/echo:0.3.8", caller_provider_contract_id="wasmcloud:httpserver", caller_provider_id="VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", caller_provider_link_name="default", host="ND7L3RZ6NLYJGN25E6DKYS665ITWXAPXZXGZXLCUQEDDU65RB5TVUHEN", job="wasmcloud-host", lattice="default", operation="HttpServer.HandleRequest"}
Provider metrics will likely need to wait until the wRPC work is finished. Add initial support for metrics enable OTEL logs support OTEL traces end-to-end<csr-unknown/>
<csr-unknown/>
<csr-unknown/>

## 0.7.0 (2024-08-23)

<csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/>
<csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-53a312c3c35014e1b337a45a96373b81512bc113/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/>
<csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/>
<csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/>
<csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/>
<csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-3048567a115618599336d4c1255fc6324c04a203/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>

### Chore

 - <csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/> gate import behind feature
 - <csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/> Bump opentelemetry-* crates and tracing-opentelemetry to latest version
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/> bump to 0.3.0
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-53a312c3c35014e1b337a45a96373b81512bc113/> bump to 0.2.0
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/> Update the default OTLP HTTP port to match the current spec
 - <csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/> Remove check for default OpenTelemetry traces path
 - <csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/> Normalize service.names to use kebab-case
 - <csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/> address clippy warnings
 - <csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/> clean-up imports
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting

### New Features

<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>
<csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/>
<csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/>

 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e28361935ad3b09d46658488e813c809522317bf/> add support for flame graphs
 - <csr-id-1b076b3479874dbc2f7e575fcee65bab66bd056d/> use `tracing-appender`
   Avoid locking whole process on each logging statement
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef

### Bug Fixes

 - <csr-id-3d2cce2e3dcbd23bb7d29c3a4c126650df889b0d/> enable core otel features
 - <csr-id-8b66966e03e5d9c2fa64fb2480e9264f3f9e2f55/> include rustls-tls feature for reqwest
 - <csr-id-a10e171e16e08f16e21ad07fff99343b10363fc9/> unused metrics functions
 - <csr-id-f38f5510fc53ea83a94378851a02c3800444388f/> fix compilation issue in tracing
 - <csr-id-8d345114fbd30a3f6784d2b22fa79f1c44f807c5/> split directives before trying to parse
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-46b441d1358fd0ee349bf1dfc87236c400cb4db1/> reduce verbosity of nats logs
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/> pub the context mod only with the otel feature enabled

### Other

 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0

### Refactor

 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-3048567a115618599336d4c1255fc6324c04a203/> simplify registry construction
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers

### Style

 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports

### New Features (BREAKING)

 - <csr-id-8ad2cde49cb52872af4c9753be7c422092ae56ee/> add trace_level option
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Bug Fixes (BREAKING)

 - <csr-id-ca8959ab707196cb061b7b732c4ed9e3a92e2c88/> return Dispatch from configure_tracing()
   This commit returns Dispatch from `configure_tracing()` rather than
   attempting to call `set_global_default()`[0].
   
   `set_global_default` cannot safely be called from libraries
   that are used in processes that will be executing (the host and a
   binar provider, in the wasmCloud case).
   
   Setting the default must be done from closer to the binar -- at the
   very least not in a shared library that both the host and provider use.

<csr-unknown>
<csr-unknown>
<csr-unknown>
<csr-unknown>
This also adds a bunch of new attributes to the existing actor metrics so that they make sense in an environment with multiple hosts. Specifically this adds:For actor to actor calls, instead of having the provider metadata it instead has the public key of the invoking actor.An example of what this looks like as an exported Prometheus metric:wasmcloud_host_actor_invocations_total{actor_ref="wasmcloud.azurecr.io/echo:0.3.8", caller_provider_contract_id="wasmcloud:httpserver", caller_provider_id="VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", caller_provider_link_name="default", host="ND7L3RZ6NLYJGN25E6DKYS665ITWXAPXZXGZXLCUQEDDU65RB5TVUHEN", job="wasmcloud-host", lattice="default", operation="HttpServer.HandleRequest"}
Provider metrics will likely need to wait until the wRPC work is finished. Add initial support for metrics enable OTEL logs support OTEL traces end-to-end<csr-unknown/>
<csr-unknown/>
<csr-unknown/>
<csr-unknown/>

## 0.6.0 (2024-07-31)

<csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/>
<csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-53a312c3c35014e1b337a45a96373b81512bc113/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/>
<csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/>
<csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/>
<csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/>
<csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-3048567a115618599336d4c1255fc6324c04a203/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>

### Chore

 - <csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/> gate import behind feature
 - <csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/> Bump opentelemetry-* crates and tracing-opentelemetry to latest version
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/> bump to 0.3.0
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-53a312c3c35014e1b337a45a96373b81512bc113/> bump to 0.2.0
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/> Update the default OTLP HTTP port to match the current spec
 - <csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/> Remove check for default OpenTelemetry traces path
 - <csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/> Normalize service.names to use kebab-case
 - <csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/> address clippy warnings
 - <csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/> clean-up imports
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting

### New Features

<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>
<csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/>
<csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/>

 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e28361935ad3b09d46658488e813c809522317bf/> add support for flame graphs
 - <csr-id-1b076b3479874dbc2f7e575fcee65bab66bd056d/> use `tracing-appender`
   Avoid locking whole process on each logging statement
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef

### Bug Fixes

 - <csr-id-8b66966e03e5d9c2fa64fb2480e9264f3f9e2f55/> include rustls-tls feature for reqwest
 - <csr-id-a10e171e16e08f16e21ad07fff99343b10363fc9/> unused metrics functions
 - <csr-id-f38f5510fc53ea83a94378851a02c3800444388f/> fix compilation issue in tracing
 - <csr-id-8d345114fbd30a3f6784d2b22fa79f1c44f807c5/> split directives before trying to parse
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-46b441d1358fd0ee349bf1dfc87236c400cb4db1/> reduce verbosity of nats logs
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/> pub the context mod only with the otel feature enabled

### Refactor

 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-3048567a115618599336d4c1255fc6324c04a203/> simplify registry construction
 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers

### Style

 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports

### New Features (BREAKING)

 - <csr-id-8ad2cde49cb52872af4c9753be7c422092ae56ee/> add trace_level option
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Bug Fixes (BREAKING)

 - <csr-id-ca8959ab707196cb061b7b732c4ed9e3a92e2c88/> return Dispatch from configure_tracing()
   This commit returns Dispatch from `configure_tracing()` rather than
   attempting to call `set_global_default()`[0].
   
   `set_global_default` cannot safely be called from libraries
   that are used in processes that will be executing (the host and a
   binar provider, in the wasmCloud case).
   
   Setting the default must be done from closer to the binar -- at the
   very least not in a shared library that both the host and provider use.

<csr-unknown>
<csr-unknown>
<csr-unknown>
<csr-unknown>
<csr-unknown>
This also adds a bunch of new attributes to the existing actor metrics so that they make sense in an environment with multiple hosts. Specifically this adds:For actor to actor calls, instead of having the provider metadata it instead has the public key of the invoking actor.An example of what this looks like as an exported Prometheus metric:wasmcloud_host_actor_invocations_total{actor_ref="wasmcloud.azurecr.io/echo:0.3.8", caller_provider_contract_id="wasmcloud:httpserver", caller_provider_id="VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", caller_provider_link_name="default", host="ND7L3RZ6NLYJGN25E6DKYS665ITWXAPXZXGZXLCUQEDDU65RB5TVUHEN", job="wasmcloud-host", lattice="default", operation="HttpServer.HandleRequest"}
Provider metrics will likely need to wait until the wRPC work is finished. Add initial support for metrics enable OTEL logs support OTEL traces end-to-end<csr-unknown/>
<csr-unknown/>
<csr-unknown/>
<csr-unknown/>
<csr-unknown/>

## 0.5.0 (2024-06-12)

<csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-53a312c3c35014e1b337a45a96373b81512bc113/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/>
<csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/>
<csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/>
<csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/>
<csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>
<csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/>

### Chore

 - <csr-id-44d35f268e1c55a1fbb91f2bc27b43a19c4581fe/> Bump opentelemetry-* crates and tracing-opentelemetry to latest version
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/> bump to 0.3.0
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-53a312c3c35014e1b337a45a96373b81512bc113/> bump to 0.2.0
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/> Update the default OTLP HTTP port to match the current spec
 - <csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/> Remove check for default OpenTelemetry traces path
 - <csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/> Normalize service.names to use kebab-case
 - <csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/> address clippy warnings
 - <csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/> clean-up imports
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting

### Chore

 - <csr-id-c262023ea20c256686d7f1bdd1d6b21b031b55a6/> gate import behind feature

### New Features

<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>
<csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/>
<csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/>

 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef

### Bug Fixes

 - <csr-id-a10e171e16e08f16e21ad07fff99343b10363fc9/> unused metrics functions
 - <csr-id-f38f5510fc53ea83a94378851a02c3800444388f/> fix compilation issue in tracing
 - <csr-id-8d345114fbd30a3f6784d2b22fa79f1c44f807c5/> split directives before trying to parse
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-46b441d1358fd0ee349bf1dfc87236c400cb4db1/> reduce verbosity of nats logs
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/> pub the context mod only with the otel feature enabled

### Refactor

 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers

### Style

 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports

### New Features (BREAKING)

 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

## 0.4.0 (2024-05-08)

<csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-53a312c3c35014e1b337a45a96373b81512bc113/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/>
<csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/>
<csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/>
<csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/>
<csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>

### Chore

 - <csr-id-be57edb70fe783ca71c2eadc7f27d68e5712b3e7/> bump to 0.3.0
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-53a312c3c35014e1b337a45a96373b81512bc113/> bump to 0.2.0
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-cdf389bdda44fbccfb0f513d84f3737722f0a1a7/> Update the default OTLP HTTP port to match the current spec
 - <csr-id-71f8bc0a19c26cb8d2d845c69a61e7f43c409d3d/> Remove check for default OpenTelemetry traces path
 - <csr-id-2b52f083fde88b98a20dd53ba24e4ae697fcef16/> Normalize service.names to use kebab-case
 - <csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/> address clippy warnings
 - <csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/> clean-up imports
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting

### Chore

 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features

### New Features

<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>
<csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/>
<csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/>

 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs

### Bug Fixes

 - <csr-id-a10e171e16e08f16e21ad07fff99343b10363fc9/> unused metrics functions
 - <csr-id-f38f5510fc53ea83a94378851a02c3800444388f/> fix compilation issue in tracing
 - <csr-id-8d345114fbd30a3f6784d2b22fa79f1c44f807c5/> split directives before trying to parse
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-46b441d1358fd0ee349bf1dfc87236c400cb4db1/> reduce verbosity of nats logs
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/> pub the context mod only with the otel feature enabled

### Refactor

 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers

### Style

 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports

### New Features (BREAKING)

 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.


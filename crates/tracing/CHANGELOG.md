# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

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

### New Features

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

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 29 commits contributed to the release over the course of 240 calendar days.
 - 28 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
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


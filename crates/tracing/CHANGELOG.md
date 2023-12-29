# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/>
<csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/>
<csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>

### Chore

 - <csr-id-fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31/> address clippy warnings
 - <csr-id-45eea2ae0f65a0f4f403bed14feefdd67f82d0f3/> clean-up imports
 - <csr-id-cb0bcab822cb4290c673051ec1dd98d034a61546/> add descriptions to crates
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting

### Chore

 - <csr-id-859b0baeff818a1af7e1824cbb80510669bdc976/> add changelogs for host

### New Features

 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end

### Bug Fixes

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

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 15 commits contributed to the release over the course of 123 calendar days.
 - 14 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add changelogs for host ([`859b0ba`](https://github.com/connorsmith256/wasmcloud/commit/859b0baeff818a1af7e1824cbb80510669bdc976))
    - Address clippy warnings ([`fffc9bb`](https://github.com/connorsmith256/wasmcloud/commit/fffc9bb8cf42e0f5f7f03971b46dd5cdbb6d2c31))
    - Clean-up imports ([`45eea2a`](https://github.com/connorsmith256/wasmcloud/commit/45eea2ae0f65a0f4f403bed14feefdd67f82d0f3))
    - Add descriptions to crates ([`cb0bcab`](https://github.com/connorsmith256/wasmcloud/commit/cb0bcab822cb4290c673051ec1dd98d034a61546))
    - Split directives before trying to parse ([`8d34511`](https://github.com/connorsmith256/wasmcloud/commit/8d345114fbd30a3f6784d2b22fa79f1c44f807c5))
    - Proxy RUST_LOG to providers ([`691c371`](https://github.com/connorsmith256/wasmcloud/commit/691c3719b8030e437f565156ad5b9cff12fd4cf3))
    - Satisfy clippy linting ([`1a80eea`](https://github.com/connorsmith256/wasmcloud/commit/1a80eeaa1f1ba333891092f8a27e924511c0bd68))
    - Reduce verbosity of nats logs ([`46b441d`](https://github.com/connorsmith256/wasmcloud/commit/46b441d1358fd0ee349bf1dfc87236c400cb4db1))
    - Filter verbose logs ([`5ead09f`](https://github.com/connorsmith256/wasmcloud/commit/5ead09f6ee292e4923dcbfcce64ee3d6081dca2d))
    - Attach traces on inbound and outbound messages ([`74142c4`](https://github.com/connorsmith256/wasmcloud/commit/74142c4cff683565fb321b7b65fbb158b5a9c990))
    - Pub the context mod only with the otel feature enabled ([`45b0fb0`](https://github.com/connorsmith256/wasmcloud/commit/45b0fb0960921a4eebd335977fd8bc747def97a4))
    - Replace lazy_static with once_cell ([`e1d7356`](https://github.com/connorsmith256/wasmcloud/commit/e1d7356bb0a07af9f4e6b1626f5df33709f3ed78))
    - Update imports ([`a8538fb`](https://github.com/connorsmith256/wasmcloud/commit/a8538fb7926b190a180bdd2b46ad00757d98759a))
    - Construct a strongly typed HostData to send to providers ([`23f1759`](https://github.com/connorsmith256/wasmcloud/commit/23f1759e818117f007df8d9b1bdfdfa7710c98c5))
    - Support OTEL traces end-to-end ([`675d364`](https://github.com/connorsmith256/wasmcloud/commit/675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6))
</details>


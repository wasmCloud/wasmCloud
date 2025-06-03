# Enforce signed artifacts during OCI registry interactions

* Status: Accepted
* Date: 2020-11-19

## Context and Problem Statement

When a user uses `wash`, the `waSCC Shell`, should they be able to interact with provider archives or actors that are not signed? Any developer can create `WASM` files or gzipped archives with the file type `.par.gz`, but without an embedded JWT we have no way to verify the creator, origin, and safety of these files.

## Decision Drivers <!-- optional -->

* Allowing developers to interact with OCI artifacts freely without `waSCC` specific restrictions
* Confidence that artifacts `push`ed and `pull`ed with `wash` are signed
* Difficulty of signing artifacts
* Difficulty of verifying artifacts
* Inability to prevent developers from hosting their own registry with unsigned artifacts

## Considered Options

* No restriction on artifacts `push`ed or `pull`ed with `wash`
* Provider archives and waSCC actors _must_ be signed (embedded JWT) before `push`ed or `pull`ed using `wash`

## Decision Outcome

Chosen option: Provider archives and waSCC actors _must_ be signed (embedded JWT) before `push`ed or `pull`ed using `wash` because our official `waSCC` tooling should enforce our security stance. Artifacts produced for consumption by `waSCC` MUST be signed to ensure a verifiable source and verifiable attestations of capabilities for actors. `wash` is intended to make signing easier, and the difficulty of signing a provider archive or actor does not outweigh the benefits of verifiable artifacts. In the scenario where a user would like to push other types of artifacts, use of the [ORAS project](https://github.com/deislabs/oras) is encouraged.

### Positive Consequences <!-- optional -->

* Users can freely `pull` artifacts using `wash` knowing that provider archives or actor modules MUST be signed
* Artifacts `push`ed using `wash` cannot accidentally be left unsigned

### Negative Consequences <!-- optional -->

* Developers must use a tool other than `wash` to distribute unsigned WASM/provider archives

## Links <!-- optional -->

* [Suggested alternative to `wash`, ORAS](https://github.com/deislabs/oras)

# Block multiple versions of the same entity within the same lattice

| Status | Deciders | Date |
|--|--|--|
| Accepted | wasmCloud Team | 2021-10-12 |

<hr/>

## Context and Problem Statement

The question routinely arises about what to do with version upgrades or even version changes of entities within a lattice. For example, if a provider is upgraded to a new version, what happens to the existing actors that are using the old version? If multiple versions of the same provider exist in the same lattice, how do we know which one to use?

The same goes for actors. How do we rationalize routing to an actor if multiple versions of it exist in the same lattice?

## Considered Options

* Support the multi-version behavior and randomly choose targets
* Support multi-version behavior with a fundamental change to lattice RPC semantics
* Disallow multi-version behavior

## Decision Outcome

_**Disallow multi-version behavior**_

### Positive Consequences

An entire suite of failure scenarios are prevented with this decision. It would be far too easy for developers and production environments to get into scenarios with unpredictable behavior through arbitrary routing to targets. In short, _we need to block the ability for sequential requests to be routed to different versions of the same entity_, because there is no way to _safely_ support this today.

### Negative Consequences 

For those who want to be able to run multiple concurrent versions of the same thing, they will need to adopt a strategy that utilizes multiple lattices. Rather than supporting multiple versions in the same lattice, developers can use two lattices for things like blue/green, canary, and concurrent versions during upgrade.



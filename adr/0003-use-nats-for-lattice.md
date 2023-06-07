# Use NATS as the foundation for lattice

## Status
Accepted

## Context and Problem Statement

_Lattice_ is the self-healing, self-forming network extension to the WebAssembly actor runtime host. 
Lattice needs to be able to create flattened network topologies regardless of the number of intervening hops,
routers, gateways, clouds, and physical infrastructures. As **the** core of the networking infrastructure, whatever
lattice is built with must be [_boring_](https://mcfunley.com/choose-boring-technology) [^1], _flexible_, _self-maintaining_, _secure_, _reliable_, _fast_, and _easy to use_.

## Decision Drivers <!-- optional -->

* Impact on developers (for good or bad)
* Impact on operations and support
* Cost of running, cost of associated infrastructure
* Reliability, Stability, Performance, and other quantifiable ratings
* Extensibility and support for future "unknown" use cases
* Developer, support, and open-source community size and engagement level

## Considered Options

* NATS
* Apache Kafka
* RabbitMQ
* Redis
* Write our Own
* Cloud-Coupled/Proprietary (SQS, Google, Azure, etc)

## Decision Outcome

Chosen option: **NATS**, because it was by far the simplest yet most powerful, flexible, easy-to-run, and easy-to-support distributed messaging system evaluated. The power and flexibility brought to bear by "leaf nodes"[^2] alone is a compelling enough argument to use NATS. 

### Positive Consequences <!-- optional -->

* A truly powerful foundational building block on which we can build all kinds of enhancements and features
* Leaf nodes
* Access to NGS with the same set of APIs, libraries, and primitives as used internally
    * Leaf nodes + NGS == <3
* Drop-dead simple developer experience (just start the `nats` binary)
* Amazing _decentralized_, account-based, multi-tenant security system that originally inspired our use of JWTs in `wascap` for signing actor modules.
* Streaming is available as an easy add-on if we want it, but won't get in the way if we don't

### Negative Consequences <!-- optional -->

It is possible that, by choosing NATS, we might miss out on some powerful persistence technology available 
in a heavy-duty (possible reading: _bloated_) broker, but at this point we don't think our needs warrant the
additional baggage.

NATS is also not quite as well-known as Apache Kafka and RabbitMQ, though we think it should be. We might have to defend
our decision to use NATS more often than if we had picked a different broker, but it's worth it.

## Pros and Cons of the Options <!-- optional -->

### NATS

NATS is a small, lightweight message broker that has been designed from the very beginning to be low-maintenance, easy to configure, flexible, and fast. It remains one of the most "cloud native" messaging systems we've encountered.

* Good, because "it just works"
    * In use in production without a single moment of downtime for some extremely critical use cases.
* Good, because as of 2.0 it offers a decentralized security model that is ideal for federation, multi-tenancy, and giving tenants power and flexibility. 
    * This same decentralized model means an entire NATS cluster can be compromised without the loss of a single private key
* Good, because it uses a simple protocol that doesn't force us to use specific schemas or serialization patterns.    
* Good, because of leaf nodes and interoperability with **NGS**.
* Good, because of broad community acceptance, use, and support.
    * NATS is a part of the CNCF and is popular in that community.
* Good, because streaming and the complexity that comes with it is opt-in. We don't bear that burden until/unless we need it.
* Good, because it's incredibly easy to run during development and manage in production
* Bad, because we might decide we need the ultra-heavy streaming support of something like Kafka? (honestly this is a stretch and we doubt we'll need this in the foreseeable future)

### RabbitMQ

RabbitMQ is written in Erlang and is used to support all kinds of incredibly large-scale production workloads. _"Nobody would get fired"_ for picking Rabbit.

* Good, because you can programmatically control queues and partitions
* Good, because it is relatively light-weight and easy to run in development
* Bad, because it still requires post-installation operations to manage via API or console
* Bad, because it has a relatively limited/narrow security story

### Apache Kafka

Kafka is the 300 pound elephant of the brokers we evaluated. It has a nearly infinite list of extensions and tie-ins
to massive numbers and sizes of ecosystems. It has broad support and is running in production at
just about every scale and configuration imaginable.

* Good, because it is one of the de-facto choices for message brokers/streaming systems
* Good, because it has robust streaming and persistence capabilities
* Good, because it has well-known scaling characteristics
* Bad, because someone has to manually perform stream repartitioning to adapt to new scaling patterns (this goes against the "hands off" and "boring" maintenance needs)
* Bad, because it's also a _huge pain_ to install and manage locally, as well as manage _properly_ and _well_ in production. Without the right staff, we'd need a hosted solution to take on that burden.
* Bad, because the "embarrassment of riches" of options, configurations, plug-ins, and everything else makes it hard for developers to work with.
* Bad, because though everything is pluggable, its security model isn't as powerful as what we want (multi-tenancy isn't a native concept without add-ons)

### Redis

In recent years Redis has become a kitchen sink of services, providing far more than just a distributed key-value store with optional persistence.

* Good, because you get a bunch of stuff "for free", like the aforementioned key-value store and access to a community of extensions and plugins (e.g. graph databases, geo support, etc)
* Good, because Redis is easy for developers to use. The server "just works" locally and is usually relatively low maintenance in production
* Bad, because the message broker part (channels) is a relatively simple "add-on" to Redis
* Bad, because the security model isn't decentralized or as robust as we'd like
* Bad, because it's too easy (and tempting) to put state and messaging in the same place, which can cause problems given the aforementioned security concerns.

### Write our Own

Included here as an option for the sake of showing all possibilities. This would involve us creating our own
networking code, likely starting from TCP/UDP low-level stuff and building up from there.

* Good, because we could potentially have the most control over networking behavior compared to all other message brokers
* Bad, because our [hedgehog](https://www.jimcollins.com/concepts/the-hedgehog-concept.html) is not making message brokers, it's making a distributed actor system that runs WebAssembly actors.
* Bad, because it would be foolish to think that we could create something on our own that rivals any of the other options in this list
* Bad, because even if we had the skills and resources to create our own "lattice from scratch", it would take so much time that we would have to drop everything from our backlog to accommodate the effort.

### Cloud-Proprietary

This is only included for the sake of completeness. We only briefly considered this before deciding against it.

## Links <!-- optional -->

* [NATS](https://nats.io)
* [Synadia](https://synadia.com/), the company that hosts **NGS**, a _"global messaging dial-tone"_
* Whitepaper [A study on modern messaging systems-Kafka, RabbitMQ, and NATS Streaming](https://arxiv.org/pdf/1912.03715.pdf)

---
[^1]: From the article, _"Technology for its own sake is snake oil"_
[^2]: [https://docs.nats.io/nats-server/configuration/leafnodes](https://docs.nats.io/nats-server/configuration/leafnodes)

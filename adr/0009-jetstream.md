# Use JetStream for Distributed Lattice Cache

* Status: _Accepted_

## Context and Problem Statement

During the course of normal operation, wasmCloud hosts need to share certain information; metadata about the contents and operation of the _lattice_. This includes information such as a lookup table of JWT claims, a lookup table that maps OCI URLs to public keys, and a list of link definitions.

This information needs to be shared with all hosts in a lattice at all times, and it needs to withstand things like partition events and temporary connectivity loss, as well as expected events like the termination and starting of individual host processes.

The current solution to this problem is not reliable, doesn't scale beyond simple installations and, worse, can quite easily result in "split-brain" problems where different hosts have different views of the same set of information, potentially causing runtime failures in production.

## Decision Drivers 

* Reliability
* Developer Experience
* Predictability
* Scalability
* Resiliency
* Enterprise suitability

## Considered Options

* Leave As-Is
* Use Redis
* Use memcached/etcd/consul or their ilk
* Use NATS JetStream

## Decision Outcome

Chosen option: _**Use NATS JetStream**_, because this is the only option that actually satisfies all of our runtime production requirements while still enabling a simple developer experience and _not_ burdening the developer by requiring the installation of additional software.

### Positive Consequences 

* Gain a "free" external source of truth for the lattice cache without requiring developers to install additional products
* Gain access to all of the JetStream features, including defining replication, clusters, persistence, and more
* Ability to provide a reasonable default while allowing organizations and enterprises to define their own stream configuration

### Negative Consequences

* Requires developers to enable the JetStream feature when starting NATS servers for development (goes to developer experience)

## Pros and Cons of the Options

### Leave As-Is

Continue using _naive state replication_ by simply emitting _at most once_ messages in fire-and-forget fashion into the lattice.

* Bad, because message loss will result in cache drift/split brain
* Bad, because at least one host must remain up at all times to maintain state
* Bad, because it is intolerant of partition events

### Use Redis

Communicate with a Redis server (or cluster) and store lattice cache metadata in Redis keys

* Good, because the information in the cache would be durable
* Good, because if properly clustered, it could tolerate certain kinds of partition events
* Bad, because developers and operators now have an entirely new product to maintain even to get a simple "hello world" sample running


### Use memcached/etcd/consul/etc

Use one of these other servers that are often optimized for high-efficiency, in-memory operation.

* Basically the same Good/Bad arguments as Redis

### Use NATS JetStream

Rely on the features of streams in NATS JetStream to manage the lattice shared metadata

* Good, because developers are already using NATS, they simply have to enable JetStream (requires no additional download)
* Good, because we can default to a quick, easy, in-memory store while optionally supporting more complex configurations

## Links 

* Documentation - [About JetStream](https://docs.nats.io/jetstream/jetstream)



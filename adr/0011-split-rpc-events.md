# Split RPC Events from Regular Events on Event Topic
Allow all cloud events to remain published on `wasmbus.evt.{lattice}` but move the publication of invocation 
success and failure to a separate topic, `wasmbus.rpcevt.{lattice}`.

## Context and Problem Statement

There are two problems with the status quo, one can causes crashes while the other is one of organization/optimization. 

The first issue is a fairly niche case. If an actor is using the `wasmcloud:messaging` capability provider to subscribe to a `wasmbus.evt.*` topic, then each time the capability provider delivers (or fails to) a message to the actor, the host emits an _invocation succeeded/failed_ event, also on `wasmbus.evt.*`. This will then trigger that same actor to receive the invocation event, which will in turn emit another invocation event, and so on until infinity or the host crashes. To avoid this death spiral, we need to not emit invocation events on the same topic as the other events.

The second issue is just one of organization and traffic optimization. In a steady state lattice, invocation events will happen exponentially more than any of the other ones (followed closely by heartbeats). If we're not interested in the high-traffic events, our code/actor shouldn't have to manually filter them out and can instead bypass them with a subscription wildcard.

## Decision Drivers

* Despite the limited nature of the main use case, this will cause a crash
* Optimization and efficiency of consumers of lattice events


## Decision Outcome

Split event publication so RPC (invocation) events are published on `wasmbus.rpcevt.*` while all other events stay on `wasmbus.evt.*`

### Positive Consequences 

* No crashes
* More optimized consumer flow


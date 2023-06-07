# Actors are Stateless

## Status

Accepted

## Context and Problem Statement

Many actor frameworks have built-in, language-idiomatic support for state. Actors can maintain private state or use special APIs to maintain distributed state. Should our actors be stateless or stateful?

## Decision Drivers <!-- optional -->

* Adherence to apparent norms and conventions for the actor model
* Ease of use and convenience for developers
* Difficulty of implementation (distributed state)
* Degree of support for options within core WebAssembly spec
* Potential impact on system, actor, and provider scalability

## Considered Options

* Intrinsic support for actor state in the actor API
  * Lattice maintains actor state for all actors (resilient state)
* Actors are inherently stateless - must get state from a capability provider

## Decision Outcome

Chosen option: "stateles actors", because we feel that stateless actors are the best option to provide the flexibility, ease of use, and scalability while adding the least amount of complexity and "new primitives" to the underlying framework. For more details, see below.

### Positive Consequences

* No introduction of new primitives or special one-off cases.
* Easier to develop and maintain

### Negative Consequences

* Potential documentation and public relations friction in explaining why this framework doesn't support in-module state management.

## Pros and Cons of the Options

### Stateless Actors

The term "stateless", as usual, needs to be clarified within this context. State in the case of an actor defined within a WebAssembly module means values that exist in that module's linear memory. By saying "stateless", what we're actually saying is that the host runtime provides _no guarantees_ that any data in linear memory will remain unaltered between function calls, or will be restored when an actor is restarted.

Stateless actors do not rely on values to remain in their linear memory, and they _must_ rely upon bound capability providers to access any persistent state.

Even **Dapr**, which claims to have _stateful actors_, is really doing the same thing our runtime does--exposing a service that communicates with _external state_ [^1] .

* *Good*, because of simplicity. If you can't get state without a capability provider, then it makes the building blocks and boundaries clearer to everyone involved.
* *Good*, because of ease of implementation. The runtime itself does not have to deal with support for resilient state.
* *Good*, because the explicit opt-in nature of using a state capability provider means developers have explicitly chosen the _nature_ of the state on which their actor relies, without having to choose the _vendor_ of that state provider. Explicitly choosing key-value versus SQL versus Graph lets the code be more self-documenting with regard to the attributes of dependent state.
* *Good*, because being direct about the stateless opinion means that there is no illusory facade that gives developers the impression they're working with local/in-process state when they are really working against external state. This false impression allows developers to make assumptions about how the system works that can be detrimental to performance or worse, create difficult to track bugs when the system runs under load.
* *Bad*, because developers not using languages that make it difficult to create global mutable state may inadvertently code actors that make incorrect assumptions about state availability.
* *Bad*, because developers looking for a "kitchen sink" framework may be disappointed to see an opinionated one that mandates external state or does not go to great lengths to pretend that external state is idiomatic internal state.

### Stateful Actors

Stateful actors in this context refer to actors that operate on state as though it is guaranteed to remain available across instance restarts of a given actor. This comes with the assumption that the substrate or system in which the actor is running is deeply (e.g tightly coupled) aware of, managing, and supplying all state for all actors.

Whether it's an illusion to the developer or not, state does not appear to be external, and is often manipulated using basic programming language idioms.

An example of the kind of complexity sprawl we did not want to take on for a lightweight lattice can be found in Akka[^2] persistence.

* *Good*, because it can be argued that stateful actors provide a simpler, more friendly developer experience than stateless.
* *Bad*, because relying on _implicit state management_ can often lead developers to stop thinking about distributed problems as though they are distributed, leading to design and architectural problems that may not surface until production.
* *Bad*, because creating the supervisory substrate to manage state resilience, persistence, replication, and availability to actors is a very complex problem, and is a potential wellspring of bugs and maintenance issues for framework maintainers.

## Links

Links, references, and footnotes

[^1]: [DAPR State Management](https://github.com/dapr/docs/tree/master/concepts/state-management)

[^2]: [Akka Persistence](https://doc.akka.io/docs/akka/current/typed/index-persistence.html)

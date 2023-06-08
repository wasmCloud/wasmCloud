# Actor-to-Actor Calls Allowed by Default

In this decision record we discuss the choice of allowing actors to call operations on other actors
by default.

## Context and Problem Statement

wasmCloud is a _zero trust_ security model, and so when people look at the default security policies they want
to know whether it is appropriate for actors to be allowed to call each other by default. For comparison, actors
cannot interact with capability providers without having an established link definition between them created at
runtime.

The rest of this ADR discusses the options and decisions, as well as the tradeoffs involved, in determining whether
or not actors should be allowed to communicate directly by default.

## Considered Options

* Allow by Default
* Deny by Default
* Create Explicit Actor Calling Policies

### Allow by Default

This option essentially provides no _built-in_ rules preventing communication between actors. Developers and administrators can still use the [policy service](https://wasmcloud.com/docs/production/secure/policy) to create their own policies.

### Deny by Default

This option is the reverse of the preceding: _deny_ all actor-to-actor communication by default. The current state of **authorizer plugins** within wasmCloud only serves to further _restrict_ what can be done. You cannot currently use an authorizer plugin to _relax_ security constraints. This means that if we denied actor-to-actor calling, it would be impossible to create allow-list exceptions without adding new code to the host core.

### Create Explicit Actor Calling Policies

Somewhere in the middle between the **deny all** and **allow all** options would be the use of an explicit policy where administrators could determine at runtime which actors can invoke which other actors.

This is actually supported right now through the use of the **authorizer** trait. One could quickly and easily create an authorizer that defers the policy decision to something like _Open Policy Agent_ and implement this functionality.

## Decision Outcome

We chose to allow actor-to-actor communication by default.

## Pros and Cons of the Options

### Allow All

There might be some concern that allowing all communication by default might constitute a security risk in an otherwise "zero trust" network. This is a legitimate concern, but we should put it in the appropriate perspective.

In order for an actor to communicate with another actor, it must have been loaded by the host, which means it passed an initial "can we load this actor?" test. Custom authorizer plugins make it possible to verify the issuer of an actor against a well-known allow list.

In addition, that same actor's JWT must be valid, not expired, and have a valid set of claims.

One common use case for intra-actor communication is when an an actor exposes one or more operations as _services_: operations that provide a barrier abstraction over some functionality. The actor handling that operation might have a connection to a key-value store, a relational database, a graph database, or any number of runtime-bound capabilities.

As a concrete example, let's consider an actor that exposes a `QueryInventory` operation that, when invoked, queries the current amount of stock remaining for a given `SKU`. This is a classic service abstraction and, under the rules and patterns for service design, no other actor should have direct access to this key-value store. As such, _client_ or _consumer_ actors must go through the _service actor_ to get the data they need.

This isn't a privilege escalation issue because the service actor can never do more than it is allowed to do, and the consumer actor can never ask the service to do more than it has permission to do. In other words, the worst case scenario is a client actor can send bad input to the service actor. _Privilege escalation_ is not possible here.

We feel the tradeoff for this pattern is well worth the decreased friction and improved developer experience created by allowing all actors to communicate _by default_ (remembering that plugins can further restrict that later).

### Deny All

As mentioned in earlier sections, if we were to adopt a policy of denying all actor-to-actor communications by default, then there would be no way to create exceptions to this policy in the current codebase. We would essentially be declaring actor to actor communication as an unsupported feature.

Adding additional security constraints to communication of any kind, whether it is actor-to-actor or actor-to-provider, should be done through the security authorizer plugin facility.

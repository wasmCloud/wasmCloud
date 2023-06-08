# Smallest Unit of Tenancy is the wasmCloud Host Struct

This decision record covers the pros and cons and ultimate decision regarding multi-tenancy within
a wasmCloud host.

## Context and Problem Statement

When running wasmCloud, people often need to run multiple copies of the same actor or capability provider. Running multiple copies of the capability provider with different link names gives us the ability to have multiple message brokers per actor, or multiple key-value stores, etc.

When we run multiple copies of the same actor within the same host, we assume that these copies are _solely_ for horizontally scaling actors, and _not_ for multi-tenancy.

An **inviolable** rule of wasmCloud's zero-trust policy is that we absolutely cannot ever allow one actor to compromise the security of another. We can't allow the generation of a link definition at runtime that could create a potential data exfiltration or PII compromise situation, or even enable the sharing of seemingly harmless data.

Given the state of our documentation, people looking for multi-tenant solutions might not have sufficient context to make the right architectural decisions, and so this ADR needs to make clear where the tenancy boundaries are so that they can in turn be documented clearly at [wasmcloud.com](https://wasmcloud.com).

## Considered Options

* Keep as is (tenant-per-host)
* Add a group/tenant ID to actor instances

### Tenant Per Host (Status Quo)

This option basically just keeps things as is, but requires us to more clearly document the architectural patterns surrounding multi-tenancy.

### Add Group/Tenant ID to Actor Instances

This option follows the lead of the capability provider "compound key" by adding a `tenant_id` (or `group_id`) to the actor instance.

This would allow two actors with the same public key to reside in the same host at the same time, but belonging to two different logical groups.

These logical groups could be tenants or customers, but they could be any other form or arbitrary segmentation as well. Configuration between an actor and a capability provider would then include both the source actor's public key and the source actor's group/tenant identifier. For more information on all of these details, see the [RFC](https://github.com/wasmCloud/wasmCloud/issues/195) issue.

## Decision Outcome

We chose to keep the status quo, where the smallest unit of tenancy is a single host. Two identical actors cannot exist in the same host at the same time with two different link definition sets (configurations).

### Real Usage Example

A third party integrator runs a wasmCloud host on a node (server). This server is shared by multiple customers. A customer loads and starts the `echo` actor on the host and configures the HTTP link definition. A second customer then attempts to load and start the `echo` actor on the host, but this operation fails with an error message stating that the echo actor is already running.

_This is functioning as designed_.

This can be a very confusing scenario if wasmCloud consumers and integrators are unaware of our multi-tenancy rules.

## Pros and Cons of the Options

### Status Quo

The obvious benefits of this option is that we don't have to do any work to make this happen. We will have to add to our existing documentation whether we choose the status quo or the group identifier option, so we're not considering documentation updates as work associated with one or the other.

The downside to this option is that it is possible for people to assume that you can place as many duplicate actors for as many different configuration sets (e.g. tenants) as possible with no negative consequence. This assumption can lead to runtime failures like link definition overriding, attempts to re-use existing ports, link definition failure, actor start failure, and more. All of these result in difficult to troubleshoot error messages that can consume support time from the core team and maintainers.

### Add Group ID to Actor Instances (Multi-Tenancy per Host)

The benefit to this option is that with a moderate code change to the wasmCloud host, developers could place as many actors for as many tenants/customers/arbitrary groups as they like all within a single host.

The major downside to this option, and one of the largest deciding factors, is security. While wasmCloud has multiple layers of zero-trust security, if multiple tenants are running within the same host, extra care must be taken to ensure that under no circumstances can one tenant ever be able to examine or exfiltrate data or operations from another.

While it might be possible to ensure that this security requirement has been met through unit tests, acceptance tests, and routine penetration tests, the bottom line is that most enterprises would be uncomfortable with this scenario, regardless of the security tests we have running. The risk of accidental data sharing is too great.

In short, it's just too difficult to guarantee that there will never be a single edge case that can be exploited within a multi-tenant wasmCloud host. Therefore, we have decided against supporting this option.

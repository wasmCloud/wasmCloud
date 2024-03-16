# Contributing Guide

The contributing guidelines for this repository follow the same guidelines that are specified for the wasmCloud organization. It's recommended that you read those [rules](https://github.com/wasmCloud/wasmCloud/blob/main/CONTRIBUTING.md) as well as the repository-specific ones below.

As detailed in the [Community Providers RFC](https://github.com/wasmCloud/wasmCloud/issues/261), there are two distinct categories of capability providers that can be contributed: first-party and community.

## Contributing a wasmCloud First-Party Capability Provider
wasmCloud first party capability providers live in this repository along with a guarantee that the wasmCloud maintainers and the original author will keep the providers up-to-date. These capability providers generally meet the following criteria:
- Submitted by an active wasmCloud user and contributor
- High level (80%+) code coverage with tests
- Comes with example components and scripts to show usage
- Uses or implements connectivity to a popular service (e.g. Redis for the KeyValue contract). This inherently subjective and will be weighted less towards the decision than the other criteria.

In addition to the above criteria, rigor in code review and standards will be applied when contributing a provider in this way. If you're contributing a first-party capability provider, open a [pull request](https://github.com/wasmCloud/capability-providers/pulls) with your source code as well as adding an entry into the [README.md](https://github.com/wasmCloud/capability-providers/blob/main/README.md).

## Contributing a Community Capability Provider
Community capability providers are linked to in the README.md of this repository along with the promise that the original author will keep the provider reasonably up-to-date with proper semantic versions. These capability providers generally meet the following criteria:
- Submitted by a wasmCloud user at any level of contributing including first time contributions
- Comes with example components and scripts to show usage
- Uses or implements connectivity to a popular service (e.g. Redis for the KeyValue contract). This inherently subjective and will be weighted less towards the decision than the other criteria.
In general, community capability providers will be readily accepted as long as they are functional and come with an easy-to-use example. Community capability providers should implement useful functionality, e.g. alternate implementations of HTTPservers or connectors to other services. Demo or one-off providers like [fake payment](https://github.com/wasmCloud/examples/tree/main/provider/fakepay) or factorial are better suited for our examples repository. To submit a community capability provider open a [pull request](https://github.com/wasmCloud/capability-providers/pulls) adding an entry into the [README.md](https://github.com/wasmCloud/capability-providers/blob/main/README.md) under the community section.
# Secrets in wasmCloud

| Status   | Deciders                                                                                                                                                                                     | Date        |
| -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------- |
| Accepted | Commenters on [#2190](https://github.com/wasmCloud/wasmCloud/issues/2190) and attendees of the [01 May 2024](https://wasmcloud.com/community/2024/05/01/community-meeting) community meeting | 01 May 2024 |

## Context

Production applications often require secrets at runtime to access internal or external systems. Up until now our only option for providing secrets was to link a component to a capability provider such as the Vault KV provider that could access secrets. This approach has several drawbacks:

- You must run a dedicated provider for ex. Vault that need their own secrets in order to connect to upstream providers.
- Secrets need to be retrieved every time a component or provider needs them which can be inefficient.
- Secrets management is tightly bound to a single provider and interface type, which means that the component's implementation is irrevocably tied to it. This violates the spirit of what wasmCloud is trying to solve for adopters and makes it difficult to develop applications that consume secrets.

## Problem Statement

We needed a better solution for managing secrets in wasmCloud. Adding secrets supports needs to: be a backwards-compatible change to avoid upgrading to 2.0, support multiple secrets backends, and allow Wasm components and capability providers to access secrets at runtime securely without storing or transmitting those secrets in plaintext.

## Decision Drivers <!-- optional -->

- Secrets are a key feature of application platforms.
- Up until wasmCloud 1.1, secrets had to be exposed in plaintext in order to work with wasmCloud applications.
- Secrets were the most requested wasmCloud feature in the community Q2 roadmap session.

## Considered Options

- Leave security of secrets up to the security of NATS and TLS
- Require secrets to be stored in a secrets backend and accessed via a capability provider at runtime
- Support a specific secrets backend (e.g. Vault), and tie wasmCloud to that secrets backend.
- **Support secrets as a first-class primitive in wasmCloud with a pluggable backend API**

## Decision Outcome

We've chosen to support secrets as a first-class primitive in wasmCloud with a pluggable backend API. This will enable wasmCloud to adhere to a very simple, pluggable, API, and support any and all secrets backends with a lightweight secrets-backend. This decision provides the best experience to users now and in the future, given that wasmCloud does not manage secrets directly and will not be a blocker for developers & companies that have their own secrets management solution.

We've designed the `wasmcloud:secrets` interfaces specifically as a generic secrets interface that can be upstreamed in the future. It consists of two interfaces, `store` and `reveal`, which allow the design of interfaces that pass around a _resource_ handle to a secret rather than the value itself. Then, the component or provider that actually needs to use the secret can reveal and use the secret when needed. Interfaces that deal with sensitive values, personal data, passwords, etc can be specifically designed to never transmit the secret itself.

wasmCloud requests secrets over NATS at a configured `--secrets-topic` at component start, provider start, and link creation time on behalf of the component/provider. Contained in the encrypted SecretRequest is the embedded JWT of the host, the component or provider, the application that component or provider belongs to, and any additional metadata fields that the backend needs in order to authorize the request. The request/response model to/from a secrets backend is done using XKeys that are generated on-the-fly, and no keys need to be centrally managed or shared between wasmCloud and the backends. Each SecretRequest uses its own XKey and thus cannot be replayed to a secrets backend. Secrets are then stored in-memory (for components) using the [secrecy](https://crates.io/crates/secrecy) crate, which zeroes out the memory the secret occupied as soon as it's dropped. Because all of these requests are done at start time, rather than when the secret is accessed in the component or provider, the component or provider will fail to start if the secret is not accessible. Additionally, if the secrets backend goes down or has a temporary outage, the operation of wasmCloud applications will not be interrupted.

### Positive Consequences <!-- optional -->

- The pluggable secrets API is simple and can be implemented in any language that supports subscribing to NATS subjects and using XKeys to encrypt/decrypt payloads
- wasmCloud can support any number of secrets backends, and the feature is shipping with implementations of [NATS KV](https://github.com/wasmCloud/wasmCloud/tree/main/crates/secrets-nats-kv), [Vault](https://github.com/wasmCloud/wasmCloud-contrib/tree/main/secrets/secrets-vault), and [kube-secrets](https://github.com/wasmCloud/wasmCloud-contrib/pull/2). Developers can bring their own secrets backends/stores.
- Secrets support is a backwards-compatible non-breaking change and can be added in a minor version of wasmCloud

### Negative Consequences <!-- optional -->

- Designing a novel secrets solution that worked over the widely distributed topology that wasmCloud supports was a challenge, and took time to implement where a more tightly coupled solution would've been delivered quicker (with its own long-term costs).

## Pros and Cons of the Rejected Options <!-- optional -->

### Leave security up to NATS / Require applications to access secrets via capability provider

This was the status quo of secrets support in wasmCloud as of 1.0, and simply did not match expectations for secrets in an application platform. The transmission & storage of secrets in plaintext, even if "encrypted at rest" when using NATS KV and "encrypted in transit" when using TLS authentication, was insufficient when handling sensitive data. In addition, many teams require secrets to only be stored in their secrets store, and requiring them to be transferred to NATS KV for access was a non-starter for wasmCloud.

### Support a specific secrets backend (e.g. Vault), and tie wasmCloud to that secrets backend

This would have been the fastest option, with the least amount of flexibility for secrets. In choosing a secrets backend like NATS KV, we would still face the downside of teams that have their own solution. In choosing a secrets backend like Vault, we would tightly enforce wasmCloud hosts running in a place where they can access the Vault API, which is much less flexible than our current requirement of just connecting to NATS.

## Links

- [Original RFC](https://github.com/wasmCloud/wasmCloud/issues/2190), which is very valuable to see the detailed design
- [Secrets documentation](https://github.com/wasmCloud/wasmcloud.com/pull/526)

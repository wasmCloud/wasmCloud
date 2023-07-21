# Architectural Decision Log

This log lists the architectural decisions for wasmCloud. When an architectural decision record has arisen from an RFC or other public issues, those issues will be linked from the record.

|Status | ADR | Description |
| :--: | :--: |--|
| Accepted | [0000](0000-use-markdown-architectural-decision-records.md) | Use Markdown Architectural Decision Records |
| Accepted | [0001](0001-use-actor-model.md) | Use the Actor model for WebAssembly modules |
| Accepted | [0002](0002-stateless-actors.md) | Actors are Stateless |
| Accepted | [0003](0003-use-nats-for-lattice.md) | Use NATS as the foundation for lattice |
| Accepted | [0004](0004-enforce-signed-artifacts.md) | Enforce signed artifacts during OCI registry interactions |
| Accepted | [0005](0005-security-nkeys.md) | Flexible security foundation based on ed25519 PKI |
| Accepted | [0006](0006-actor-to-actor.md) | Actor-to-actor calls are allowed by default |
| Accepted | [0007](0007-tenancy.md) | The wasmCloud Host is the smallest unit of tenancy |
| Accepted | [0008](0008-embedded.md) | wasmCloud Host will not run on embedded devices w/out an OS |
| Accepted | [0009](0009-jetstream.md) | Use NATS JetStream for distributed cache |
| Accepted | [0010](0010-otp.md) | Use Elixir/OTP for the main cloud host runtime |
| Accepted | [0011](0011-split-rpc-events.md) | Separate RPC/invocation events from regular events on wasmbus |
| Accepted | [0012](0012-rfc-management.md) | RFCs are Managed in GitHub and Completed As ADRs |

For new ADRs, please use [template.md](template.md).

General information about architectural decision records is available at [https://adr.github.io](https://adr.github.io)

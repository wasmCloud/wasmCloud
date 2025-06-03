# RFCs are Managed in GitHub and Completed As ADRs

| Status   | Deciders                                         | Date       |
| -------- | ------------------------------------------------ | ---------- |
| accepted | wasmCloud community call members and maintainers | 07-20-2023 |

## Context and Problem Statement

RFCs in wasmCloud were unclear in terms of status of acceptance, implementation, when they are ready to be worked on. Additionally, it's difficult to tell when an RFC is completed, or if it has been abandoned or superseded by another RFC. This ADR proposes a new process for managing RFCs and their status.

## Decision Drivers <!-- optional -->

- I personally have closed a handful of RFCs months after they have been implemented or abandoned
- I know what RFCs are in progress, but I do not know their status and the RFC itself does not communicate that
- Very few RFCs are contributed by community members and/or worked on by community members.

## Considered Options

- Continue with the current process
- Manage RFCs in GitHub with clear labels and issue templates to communicate status
- Look for a different tool to manage RFCs

## Decision Outcome

We chose to modify the way we propose RFCs from beginning to end, starting with the RFC issue template and ending with the ADR that closes the RFC. Inbetween, we will communicate the status of RFCs using the following labels:

1. `rfc-proposed` - For when an RFC is newly proposed
1. `rfc-discussion-needed` - For when active discussion is going on in the RFC and it's not scoped enough for work to begin
1. `rfc-accepted` - For when an RFC is adequately scoped and ready for implementation. It's up to maintainers to decide if an RFC is scoped enough for work, and affected projects should be indicated.

As an additional note, `rfc-accepted` combined with `good-first-issue` should be a great place to begin contributing meaningful features to wasmCloud.

Additionally, the condition to close an RFC as completed is a written ADR that is contributed to this (wasmcloud/wasmcloud) repository. This ADR is the first example of such an ADR.

### Positive Consequences <!-- optional -->

- It is easier to propose RFCs for all contributors
- RFCs have a clear definition of when they are ready for implementation and when they are completed.
- Completed RFCs as ADRs leaves a clear decision log of work and serves as an easy place to see more significant decisions in the project.

### Negative Consequences <!-- optional -->

- A formal process here may overcomplicate things
- Changes to this process, including labels, may inspire changes to this ADR which is heavyhanded

## Links <!-- optional -->

- [Original RFC](https://github.com/wasmCloud/wasmCloud/issues/381)
- [Issue Template](https://github.com/wasmCloud/wasmCloud/pull/382)

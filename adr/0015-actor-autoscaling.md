# Actor Autoscaling & Scale to Zero

| Status   | Deciders                                                            | Date       |
|----------|---------------------------------------------------------------------|------------|
| Accepted | [RFC Commenters](https://github.com/wasmCloud/wasmCloud/issues/696) | 5 Oct 2023 |

## Context and Problem Statement

In wasmCloud, we face the challenge of managing application component scaling (actors or providers) efficiently. With the wasmtime enabling on-request instantiation with possible single digit nanosecond levels of latency, the concept of a fixed "count" of running instances has become obsolete. We aim to improve scalability and the developer experience by reimagining our approach to WebAssembly applications.

## Decision Drivers

* The need for a more efficient, scalable approach to handling WebAssembly components in wasmCloud.
* Desire to improve the developer experience by simplifying the scaling process.
* Advancements in wasmtime that allow for on-demand instantiation of components.

## Considered Options

* Modifying the **Scale** operation, deprecating the **Start** operation, and removing the `count` field in favor of `max_concurrent`. Actors will no longer run a set number of instances, but will instead be instantiated on-demand. This approach will also allow for "scale to zero" functionality, where actors are only instantiated when a message is received up to a maximum number of concurrent instances.
* Maintaining the current approach with a fixed `count` for instances.

## Decision Outcome

Chosen option: "Modifying the **Scale** operation and deprecating the **Start** operation", because this approach aligns with the latest advancements in wasmtime, eliminates the need for pre-determined instance counts, and allows for more efficient, on-demand scaling of WebAssembly components.

### Positive Consequences

* WebAssembly components in wasmCloud can be instantiated on-demand, leading to more efficient resource utilization.
* Simplifies the developer experience by removing the need to manage instance counts.
* Allows wasmCloud applications to scale more effectively based on load.

### Negative Consequences

* Required significant changes in existing wasmCloud projects to adapt to the new scaling approach.
* Could lead to initial confusion or learning curve for developers accustomed to the previous model.

## Pros and Cons of the Options

### Implementing a **Scale** operation

* Good, because it allows for on-demand instantiation of components, leading to better resource utilization.
* Good, because it simplifies the scaling process for developers.
* Bad, because it requires significant changes to existing wasmCloud projects.

### Maintaining the current approach

* Good, because it requires no changes to existing projects.
* Bad, because it does not take advantage of the latest advancements in wasmtime.
* Bad, because it maintains a less efficient scaling model based on predetermined instance counts.

## Links

* [Original RFC](https://github.com/wasmCloud/wasmCloud/issues/696)
* [Implementation PR](https://github.com/wasmCloud/control-interface-client/pull/56)

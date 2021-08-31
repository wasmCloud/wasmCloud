# Capability Providers

This repository contains capability providers for wasmCloud. The providers 
in the root level of this repository are _only_ compatible with version `0.50`
and _newer_ of wasmCloud. All of the pre-existing capability providers compatible
with `0.18` (aka "pre-OTP") or earlier can be found in the [pre-otp](./pre-otp) folder.

## First-Party Capability Providers
The following is a list of first-party supported capability providers developed by the
wasmCloud team.

| Provider | Contract | Description |
| :--- | :--- | :--- |
| [httpserver](./httpserver-rs) | [`wasmcloud:httpserver`](https://github.com/wasmCloud/interfaces/tree/main/httpserver) | HTTP web server built with Rust and warp/hyper |
| [httpclient](./httpclient) | [`wasmcloud:httpclient`](https://github.com/wasmCloud/interfaces/tree/main/httpclient) | HTTP client built in Rust |
| [redis](./kvredis) | [`wasmcloud:keyvalue`](https://github.com/wasmCloud/interfaces/tree/main/keyvalue) | Redis-backed key-value implementation |
| [nats](./nats) | [`wasmcloud:messaging`](https://github.com/wasmCloud/interfaces/tree/main/messaging) | [NATS](https://nats.io)-based message broker |

## Built-in Capability Providers
The following capability providers are included automatically in every host runtime:

| Provider | Contract | Description |
| :--- | :--- | :--- |
| **N/A** | [`wasmcloud:builtin:numbergen`](https://github.com/wasmCloud/interfaces/tree/main/numbergen) | Number generator, including random numbers and GUID strings |
| **N/A** | [`wasmcloud:builtin:logging`](https://github.com/wasmCloud/interfaces/tree/main/logging) | Basic level-categorized text logging capability |

While neither of these providers requires a _link definition_, to use either of them your actors _must_ be signed with their contract IDs.

## Additional Examples
Additional capability provider examples and sample code can be found in the [wasmCloud examples](https://github.com/wasmCloud/examples) repository.
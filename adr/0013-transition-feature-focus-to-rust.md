# Transition Feature Focus to Rust

| Status   | Deciders                                         | Date       |
| -------- | ------------------------------------------------ | ---------- |
| accepted | wasmCloud Maintainers | 08-02-2023 |

This ADR is confirmation of the decision(s) made in RFC [#324](https://github.com/wasmCloud/wasmCloud/issues/324).

## Context and Problem Statement
Up until very recently, the only fully supported wasmCloud host runtime was the OTP host. As the RFC discusses, there is now a shared Rust crate that is used by the OTP host for wasm functionality. This makes the OTP host little more than a facade around these features. 

As we add new features to the Rust library that are intended for many potential hosts, and not just the OTP host, we will be spending less and less time managing the OTP codebase

## Decision Drivers
The decision drivers for this are varied. One of the primary drivers is the desire to have up-to-the-minute support for the latest developments in both WASI and WebAssembly components. This functionality starts in wasmtime, and within wasmtime the Rust libraries are primary.

Further, we want to ensure that the wasmCloud host is not only easy to use for developers building actors, but we want it to be easy for people to contribute as well. It's the current opinion that the Elixir/OTP host is not as easy to contribute to as the Rust host will be.

## Considered Options
The options considered were to continue supporting the Elixir/OTP host as primary, to change to a Rust focus, or to choose a different language.

## Decision Outcome
The decision is that going forward the Rust host will be the primary wasmCloud host. Until such time as the OTP host can be retired, the Rust host will need to conform to the current lattice and host specifications so that it can run within the same lattice as the "legacy" OTP host.

## Links
- [Original RFC](https://github.com/wasmCloud/wasmCloud/issues/324)
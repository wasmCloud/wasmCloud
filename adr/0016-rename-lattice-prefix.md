# Rename Lattice Prefix to Lattice

| Status   | Deciders                                                                                        | Date        |
|----------|-------------------------------------------------------------------------------------------------|-------------|
| Accepted | Commenters & tagged contributors in [#1221](https://github.com/wasmCloud/wasmCloud/issues/1221) | 18 Jan 2024 |

## Context and Problem Statement

This ADR covers a naming change to one of our most important ideas, the lattice prefix, to just calling it a lattice. It's a simple change, but worth documenting. The simplification from lattice prefix to just lattice is aimed at reducing the cognitive burden of learning the term when you're a newcomer to the wasmCloud project, as we don't use it as a prefix rather than something more like a distributed network namespace.

## Considered Options

- Namespace
- Network Namespace
- Lattice
- Lattice Prefix
- wasmCloud Namespace

## Decision Outcome

We chose option 3, Lattice, because it's the simplest and most intuitive. The lattice is unique to wasmCloud, and is a simple way to describe the distributed namespace that wasmCloud uses to identify actors, providers, and capability providers. We'll focus on keeping terms simple and providing clear documentation to help newcomers understand the concepts.

## Links <!-- optional -->

- [Original RFC #1221](https://github.com/wasmCloud/wasmCloud/issues/1221)

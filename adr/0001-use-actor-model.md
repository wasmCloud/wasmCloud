# Use the Actor Model for WebAssembly Module Abstraction

## Status

Accepted

## Context and Problem Statement

The key unit of deployment in this project is a securely signed WebAssembly module. This is a mouthful and becomes awkward and unwieldy when describing what people will be building and the tools they'll be using.

When creating distributed systems, the need for consistent naming and terminology is a top concern for ensuring a smooth onboarding and new developer experience.

We were heavily inspired by the prior work for the past several decades on the Actor model, including implementations like those found in _Erlang/Elixir/OTP_.

## Decision Drivers <!-- optional -->

* Ease of accessibility to new developers
* Correctness of terms as they relate to distributed, concurrent systems
* Need for accurate shared language in documentation, comments, and evangelism

## Considered Options

* Actor Model
* Create a new Name / Abstraction
* No abstraction (use the word **module**)

## Decision Outcome

Chosen option: **Actor model**, because it matches with our inspiration, nomenclature, and the shared language we use to discuss the components of the overall system architecture. It also provides the right frame of reference for new developers unfamiliar with this project but who have past exposure to actor models (e.g. a _familiar paradigm_).

## Links <!-- optional -->

* [Actor Model - Wikipedia](https://en.wikipedia.org/wiki/Actor_model)

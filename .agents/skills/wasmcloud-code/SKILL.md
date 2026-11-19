---
name: wasmcloud-code
description: Code reuse and readability conventions for wasmCloud, covering reusing existing helpers and dependencies, keeping abstractions small, comment style, and grouping code. Use when implementing, refactoring, or reviewing code in any language, including tests, fixtures, and developer tooling.
---

# Code changes

Follow the core rules in [AGENTS.md](../../../AGENTS.md), including searching for existing code first.

- Prefer the standard library and existing dependencies before adding a dependency.
  Follow the [dependency policy](../../../CONTRIBUTING.md#selecting) when a new one is needed.
- Extend existing task runners, scripts, and test harnesses before introducing new infrastructure.
  Keep additions focused on the current requirement;
  avoid parallel implementations, generic frameworks, and configuration layers for hypothetical future needs.
- Extract shared code when it removes meaningful duplication between existing callers.
  Keep the abstraction small and preserve module boundaries;
  do not force unrelated code through a shared helper just because it looks similar.
- Do not leave maintainer- or reviewer-facing comments in the code.
  Comments describe what the code does and why,
  not the history of the change, the review that prompted it, or the task you were given.
- Avoid run-on blocks of code.
  Group statements semantically, separated by blank lines.
- Put the explanation of a dense chunk of code before it, not inside or after it.

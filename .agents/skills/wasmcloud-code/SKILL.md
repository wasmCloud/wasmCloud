---
name: wasmcloud-code
description: Apply wasmCloud code reuse and readability conventions when implementing, refactoring, or reviewing code, including tests and developer tooling.
---

# Code changes

- Before adding code, search for existing implementations, helpers, and similar call sites,
  including test utilities and repository tooling.
  Reuse or extend them when their behavior and ownership fit the task.
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
- Do not leak the conversation, prompt, or iteration history
  into code comments, commit messages, or pull request descriptions.
- Avoid run-on blocks of code.
  Group statements semantically, separated by blank lines.
- Put the explanation of a dense chunk of code before it, not inside or after it.

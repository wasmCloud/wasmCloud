---
name: wasmcloud-automation
description: Create or review wasmCloud CI workflows, developer scripts, task recipes, or TypeScript and JavaScript tooling; use for automation changes rather than merely running existing checks.
---

# CI and developer automation

Read the sections that match the files being changed.

## Task tooling

- Extend existing task runners and recipes before adding another tool.
- Use `cargo xtask <task>` for repository tasks.
- Prefer [Just](https://just.systems) to `make` for new task recipes.

## TypeScript

- Type code as fully as possible.
- Avoid `any`, including for parsed input.
  Take such values in as `unknown` and transform them to a known type.
- Make sure all code type checks before committing.

## CI

Scripts:

- Prefer a script that CI calls, and that is just as easy to run locally, over logic inlined in a workflow.
  - In Rust, add an `xtask` subcommand.
  - In TypeScript and JavaScript, add a file to a workspace-level `scripts/` folder.
  - In Go, add a command to the module and run it with `go run`.
- Avoid bash scripts.
- Structure a TypeScript or JavaScript script as an `async` `main()`,
  with the preamble that runs it and prints errors at the bottom of the file:

  ```ts
  async function main(): Promise<void> {
    // ...
  }

  main().catch((err: unknown) => {
    console.error(err);
    process.exit(1);
  });
  ```

- A script that is likely to run in GitHub Actions may use its logging and error facilities,
  but it must work outside GitHub Actions as well.

Workflows:

- Never write YAML lists in the inline `[a, b]` form.
  Write one item per line so that edits produce small diffs:

  ```yaml
  branches:
    - main
    - release/*
  ```

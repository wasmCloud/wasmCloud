---
name: wasmcloud-automation
description: Conventions for wasmCloud CI and developer automation, covering where scripts live, the JavaScript script structure, xtask and Makefile recipes, and workflow YAML style. Use when creating or changing files under .github/workflows/, .github/actions/, or .github/scripts/, .mjs scripts, xtask subcommands, or Makefile targets; not needed just to run existing checks.
---

# CI and developer automation

Read the sections that match the files being changed.

## Task tooling

- Extend existing task runners and recipes before adding another tool.
- Use `cargo xtask <task>` for repository tasks.
- Add `make` recipes to the existing root or `runtime-operator/` `Makefile` rather than adding a new task runner.

## JavaScript

- CI scripts are plain JavaScript ES modules (`.mjs`) that run directly with `node`.
  The repository has no TypeScript toolchain, so do not add `.ts` files or a type-check step.
- Treat parsed input as untyped, and validate it into a known shape before using it.

## CI

Scripts:

- Prefer a script that CI calls, and that is just as easy to run locally, over logic inlined in a workflow.
  - In Rust, add an `xtask` subcommand.
  - In JavaScript, add a `.mjs` file to [`.github/scripts/`](../../../.github/scripts/).
    Put shared helpers in `.github/scripts/lib/`,
    and tests next to the script as `<name>.test.mjs`, which CI runs with `node --test`.
  - In Go, add a command to the module and run it with `go run`.
- Avoid bash scripts.
- Structure a JavaScript script as an `async` `main()`,
  and run it only when the file is executed directly, so tests can import it.
  Set `process.exitCode` rather than calling `process.exit()`,
  as [`osv-gate.mjs`](../../../.github/scripts/osv-gate.mjs) does:

  ```js
  import { resolve } from 'node:path';
  import { fileURLToPath } from 'node:url';

  async function main() {
    // ...
  }

  if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    try {
      await main();
    } catch (error) {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
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

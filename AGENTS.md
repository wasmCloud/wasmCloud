# Agent instructions

Guidance for AI coding agents working in this repository.
[CONTRIBUTING.md](CONTRIBUTING.md) is the full contributor guide,
and everything in it applies to agents as well.
This file covers the points agents most often get wrong.

## Commits

- Do development work on a feature branch, never directly on `main`.
- Write titles as [Conventional Commits](https://www.conventionalcommits.org),
  for example `fix(wash-runtime): handle empty component names`.
- Keep titles under 70 characters.
- Keep commits small, and organize them around semantically distinct changes.
- Do not write a commit body unless explicitly asked to.
- When a body is asked for, wrap it at 70 characters.
- Sign off every commit with `git commit -s`.
  The DCO `Signed-off-by:` trailer is required and does not count as a body.
- Never merge `main` into a branch.
  Rebase the branch onto `main` instead, including to resolve conflicts.

## Releases

Mark every commit that introduces a breaking change in both places:

- A `!` in the title, for example `feat(wash)!: remove the --legacy flag`.
- A `BREAKING CHANGE: <what breaks and how to migrate>` footer.

## Pull requests

- Open a pull request as a draft, and iterate on it until CI is green.
- If a personal fork is configured as a remote, open the draft against the fork first.
  Open the pull request against upstream once it is ready.
- Keep the body succinct and human readable.
  A few sentences on what changed and why is usually enough.
- Do not use an excessive number of sections.
- Do not include verification methodology
  (test commands, "Validation" sections, lint or clippy results)
  unless explicitly asked to.
  The [test policy](CONTRIBUTING.md#test-policy) still applies:
  a change that needs no new tests says why in a sentence.

## Code

- Do not leave maintainer- or reviewer-facing comments in the code.
  Comments describe what the code does and why,
  not the history of the change, the review that prompted it, or the task you were given.
- Do not leak the conversation, prompt, or iteration history
  into code comments, commit messages, or pull request descriptions.
- Avoid run-on blocks of code.
  Group statements semantically, separated by blank lines.
- Put the explanation of a dense chunk of code before it, not inside or after it.

## Documentation

- Use [semantic line breaks](https://sembr.org) instead of run-on paragraphs.
- Prefer a list to a dense paragraph that covers several facets in prose.

## Rust

Style:

- Avoid single letter aliases and names,
  for example `use std::fmt as f` or `|x| ...` over a non-trivial closure body.
- Keep files under 1000 lines.
  Split a large module semantically rather than at an arbitrary point.
- Use `let ... else` for early returns, to keep the happy path unindented.
- Omit the `get_` prefix on simple getters.

API surface:

- Avoid exposing internals.
  Default to private, then `pub(crate)`, and make an item `pub` only when it is part of the intended API.
- Keep struct fields private and expose accessors,
  so that the representation can change without breaking callers.
- Do not expose types from dependencies in a public API.
  Widely used types such as `anyhow::Result` are an exception.
- Check feature branches for breaking changes with
  [`cargo-semver-checks`](https://github.com/obi1kenobi/cargo-semver-checks),
  in addition to the usual checks.

Errors:

- Use `thiserror` for custom error types that callers match on.
- Use `anyhow` for operational errors that are more widely scoped.

Before committing:

- Format with `cargo +nightly fmt`.
  The repository's `rustfmt.toml` uses unstable options, so stable `rustfmt` does not match CI.
- Lint with `cargo clippy --workspace --all-targets`,
  with the same `--features` that [`wash.yml`](.github/workflows/wash.yml) sets in `OPTIONAL_FEATURES`.

Tooling:

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

## Skills

These published skills cover topics this repository works with.
Load the ones that match the task, if your harness supports skills.
Where a skill disagrees with this file, [CONTRIBUTING.md](CONTRIBUTING.md),
or the lint and format configuration checked into this repository, the repository wins.

From [cosmonic-labs/skills](https://github.com/cosmonic-labs/skills):

- [`rust-development`](https://github.com/cosmonic-labs/skills/tree/main/rust-development):
  naming, type design, error handling, API design, and documentation for Rust code.
- [`wit-interface-design`](https://github.com/cosmonic-labs/skills/tree/main/wit-interface-design):
  designing and reviewing WIT interfaces, worlds, and packages, such as those under `wit/`.
- [`webassembly-component-development`](https://github.com/cosmonic-labs/skills/tree/main/webassembly-component-development):
  building, composing, testing, and debugging components that target WASI 0.3.
- [`wash`](https://github.com/cosmonic-labs/skills/tree/main/wash):
  using the `wash` CLI to build and run components, as in `examples/` and `templates/`.
- [`helm-charts`](https://github.com/cosmonic-labs/skills/tree/main/helm-charts):
  authoring and maintaining Helm charts, such as those under `charts/`.

From the [Bytecode Alliance](https://github.com/bytecodealliance):

- [`reduce-test-cases`](https://github.com/bytecodealliance/wasmtime/tree/main/.agents/skills/reduce-test-cases):
  minimizing a failing `.wasm`, `.wat`, `.wast`, or `.clif` input before reporting a Wasmtime bug.
- [`wasmtime-auditor`](https://github.com/bytecodealliance/wasmtime/tree/main/.agents/skills/wasmtime-auditor):
  auditing Wasmtime for bugs and vulnerabilities.
- [`jco-use`](https://github.com/bytecodealliance/jco/tree/main/.ai/skills/jco-use):
  transpiling, running, and generating TypeScript bindings for components with Jco.

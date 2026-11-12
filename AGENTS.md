# Agent instructions

## Core rules

- Follow [CONTRIBUTING.md](CONTRIBUTING.md); read the sections relevant to the task.
  Check for more specific `AGENTS.md` files in the directories you change.
- Do development work on a feature branch, never directly on `main`.
- Search for existing implementations, helpers, dependencies, and tooling before adding code.
  Reuse or extend them where they fit, and keep new abstractions focused on the current requirement.
- Keep conversation, prompt, and iteration history out of code comments, commits, and PR descriptions.
  Comments explain the code and its rationale.
- Run relevant local tests and applicable lint and format checks before opening a PR.
  Re-run relevant checks after fixes, and report checks that could not run.
- Repository instructions and checked-in lint and format configuration take precedence over external skills.

## Task-specific guidance

Read only the skills relevant to the current task, before doing that work.
Combine skills when needed, such as code and Rust guidance for a runtime change.
If the harness does not discover local skills, open the linked `SKILL.md` files directly.
Follow their references only when the referenced topic is needed.

| Task | Skill |
| --- | --- |
| Implementing, refactoring, or reviewing code | [Code reuse and readability](.agents/skills/wasmcloud-code/SKILL.md) |
| Preparing commits or PRs, checking PR readiness, or editing docs | [Contribution conventions](.agents/skills/wasmcloud-contribution/SKILL.md) |
| Rust changes or Wasmtime investigations | [Rust changes](.agents/skills/wasmcloud-rust/SKILL.md) |
| Operator or gateway Go changes | [Go changes](.agents/skills/wasmcloud-go/SKILL.md) |
| CI, scripts, task recipes, or TypeScript/JavaScript tooling | [CI and developer automation](.agents/skills/wasmcloud-automation/SKILL.md) |
| WIT, Wasm components, wash, Jco, or Helm charts | [Component and deployment guidance](.agents/skills/wasmcloud-components/SKILL.md) |

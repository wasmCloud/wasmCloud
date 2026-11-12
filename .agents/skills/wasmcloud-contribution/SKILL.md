---
name: wasmcloud-contribution
description: Apply wasmCloud commit, pull request, and documentation conventions when preparing commits or PRs, reviewing PR readiness, or editing documentation.
---

# Contribution conventions

Read only the sections relevant to the current task.
These conventions describe how to prepare a contribution; they do not authorize creating a PR or publishing changes.

## Commits

- Do development work on a feature branch, never directly on `main`.
- Write titles as [Conventional Commits](https://www.conventionalcommits.org),
  for example `fix(wash-runtime): handle empty component names`.
- Keep titles under 70 characters.
- Keep commits small, and organize them around semantically distinct changes.
- Do not write a commit body unless explicitly asked to.
- When a body is asked for, wrap it at 70 characters.
- Sign off every commit with `git commit -s`.
  The DCO `Signed-off-by:` trailer and any required `BREAKING CHANGE:` footer
  do not count as a body.
- Never merge `main` into a branch.
  Rebase the branch onto `main` instead, including to resolve conflicts.

## Releases

Mark every commit that introduces a breaking change in both places:

- A `!` in the title, for example `feat(wash)!: remove the --legacy flag`.
- A `BREAKING CHANGE: <what breaks and how to migrate>` footer.

## Pull requests

- After local tests and applicable lint and format checks pass, open a pull request as a draft,
  and iterate on it until CI is green.
  Re-run the relevant local checks after each fix before pushing it.
- If a personal fork is configured as a remote, open the draft against the fork first.
  Open the pull request against upstream once it is ready.
- Keep the body succinct and human readable.
  A few sentences on what changed and why is usually enough.
- Do not use an excessive number of sections.
- Do not include verification methodology
  (test commands, "Validation" sections, lint or clippy results)
  unless explicitly asked to.
  The [test policy](../../../CONTRIBUTING.md#test-policy) still applies:
  behavior changes need automated tests,
  and bug fixes need regression tests unless one is infeasible.
  Explain why a change needs no new tests, why a regression test is infeasible,
  or why a local check could not run.

## Documentation

- Use [semantic line breaks](https://sembr.org) instead of run-on paragraphs.
- Prefer a list to a dense paragraph that covers several facets in prose.

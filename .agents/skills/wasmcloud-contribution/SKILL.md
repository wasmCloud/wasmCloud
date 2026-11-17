---
name: wasmcloud-contribution
description: wasmCloud commit, pull request, release, and documentation conventions, covering Conventional Commits titles, DCO sign-off, breaking-change markers, draft PRs, rebasing, and semantic line breaks. Use when writing a commit message, preparing or reviewing a pull request, checking PR readiness, or editing Markdown docs such as README.md, CONTRIBUTING.md, or files under docs/.
---

# Contribution conventions

Read only the sections relevant to the current task.
These conventions describe how to prepare a contribution; they do not authorize creating a PR or publishing changes.

## Commits

- Write titles as [Conventional Commits](https://www.conventionalcommits.org),
  for example `fix(wash-runtime): handle empty component names`.
- Keep titles under 70 characters.
- Keep commits small, and organize them around semantically distinct changes.
- Add a body only when the title and diff do not make the reason for the change clear,
  as [CONTRIBUTING.md](../../../CONTRIBUTING.md#commit-message-format) asks for messages that explain what and why.
  Keep it to a few lines on why, and wrap it at 70 characters.
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

# Contributing Guide

wasmCloud projects accept contributions via GitHub pull requests. This document outlines the process
to help get your contribution accepted.

## Table of Contents

- [Contributing Guide](#contributing-guide)
  - [Table of Contents](#table-of-contents)
  - [How to Contribute Code](#how-to-contribute-code)
  - [Pull Requests](#pull-requests)
    - [PR Lifecycle](#pr-lifecycle)
      - [Documentation PRs](#documentation-prs)
  - [Reporting a Security Issue](#reporting-a-security-issue)
  - [Developer Certificate of Origin](#developer-certificate-of-origin)
  - [Support Channels](#support-channels)
  - [Semantic Versioning](#semantic-versioning)
  - [Issues](#issues)
    - [Issue Types](#issue-types)
    - [Issue Lifecycle](#issue-lifecycle)
  - [Proposing an Idea](#proposing-an-idea)
  - [Labels](#labels)
    - [Common](#common)
    - [Issue Specific](#issue-specific)
    - [PR Specific](#pr-specific)

## How to Contribute Code

1. Identify or create the related issue. If you're proposing a larger change to wasmCloud, see
   [Proposing an Idea](#proposing-an-idea).
2. Fork the desired repo; develop and test your code changes.
3. Submit a pull request, making sure to sign your work and link the related issue.

In general, most repos in the wasmCloud project have linters and other coding standards to follow.
Those standards should be followed when you contribute your code.

## Pull Requests

Like any good open source project, we use Pull Requests (PRs) to track code changes.

### PR Lifecycle

1. PR creation
    - We more than welcome PRs that are currently in progress. They are a great way to keep track of
      important work that is in-flight, but useful for others to see. If a PR is a work in progress,
      it **must** be prefaced with "WIP: [title]". Once the PR is ready for review, remove "WIP"
      from the title.
    - It is preferred, but not required, to have a PR tied to a specific issue. There can be
      circumstances where if it is a quick fix then an issue might be overkill. The details provided
      in the PR description would suffice in this case.
2. Triage
    - The maintainer in charge of triaging will apply the proper labels for the issue. This should
      include at least a `bug` or `feature` label once all labels are applied. See the [Labels
      section](#labels) for full details on the definitions of labels.
3. Assigning reviews
    - Reviewers will either be autoassigned using a CODEOWNERS file or by maintainers of the repo
      when they triage PRs, maintainers will review them as schedule permits. The maintainer who
      takes the issue should self-request a review.
    - PRs from a community member with that are any larger than 10-ish lines requires 2 review
      approvals from maintainers before it can be merged. For contributions from contributors and
      maintainers, 2 reviews are only required if the PR is large, or if the first maintainer
      requests a second review. These size and review requirements are implemented per the judgement
      of the maintainers. In the future, we may adopt a more standardized approach
4. Reviewing/Discussion
    - All reviews will be completed using GitHub review tool.
    - A "Comment" review should be used when there are questions about the code that should be
      answered, but that don't involve code changes. This type of review does not count as approval.
    - A "Changes Requested" review indicates that changes to the code need to be made before they
      will be merged.
    - Reviewers should update labels as needed (such as `breaking`, if the PR contains a breaking
      change)
    - If a comment is a nit, it should be prefaced with the text `Nit:` to indicate to the submitter
      that addressing this comment is optional
5. Address comments by answering questions or changing code
6. LGTM (Looks good to me)
    - Once a Reviewer has completed a review and the code looks ready to merge, an "Approve" review
      is used to signal to the contributor and to other maintainers that you have reviewed the code
      and feel that it is ready to be merged.
7. Merge or close
    - PRs should stay open until merged or if they have not been active for more than 30 days. This
      will help keep the PR queue to a manageable size and reduce noise. Should the PR need to stay
      open (like in the case of a WIP), the `keep open` label can be added.
    - If the owner of the PR is a maintainer, that user **must** merge their own PRs or explicitly
      request another maintainer do that for them.
    - If the owner of a PR is _not_ a maintainer, any maintainer may merge the PR. As a rule of
      thumb, we usually recommend one of the reviewers be the one to merge the PR, but this is not
      required

#### Documentation PRs

Documentation PRs will follow the same lifecycle as other PRs. They will also be labeled with the
`documentation` label. For documentation, special attention will be paid to spelling, grammar, and
clarity (whereas those things don't matter *as* much for comments in code).

## Reporting a Security Issue

Most of the time, when you find a bug in wasmCloud, it should be reported using GitHub issues. However,
if you are reporting a _security vulnerability_, please follow the guidelines outlined in our
[security process](SECURITY.md)

## Developer Certificate of Origin

As with other CNCF projects, wasmCloud has adopted a [Developers Certificate of Origin (DCO)](https://developercertificate.org/). A DCO is a lightweight way for a developer to certify that they wrote or otherwise have the right to submit code or documentation to a project.

The sign-off is a simple line at the end of the explanation for a commit. All commits need to be
signed. Your signature certifies that you wrote the patch or otherwise have the right to contribute
the material. The rules are pretty simple, if you can certify the below (from
[developercertificate.org](https://developercertificate.org/)):

```
Developer Certificate of Origin
Version 1.1
Copyright (C) 2004, 2006 The Linux Foundation and its contributors.
1 Letterman Drive
Suite D4700
San Francisco, CA, 94129
Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.
Developer's Certificate of Origin 1.1
By making a contribution to this project, I certify that:
(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or
(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or
(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.
(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

Then you just add a line to every git commit message:

    Signed-off-by: Joe Smith <joe.smith@example.com>

Use your real name (sorry, no pseudonyms or anonymous contributions.)

If you set your `user.name` and `user.email` git configs, you can sign your commit automatically
with `git commit -s`.

Note: If your git config information is set properly then viewing the `git log` information for your
 commit will look something like this:

```
Author: Joe Smith <joe.smith@example.com>
Date:   Thu Feb 2 11:41:15 2018 -0800
    Update README
    Signed-off-by: Joe Smith <joe.smith@example.com>
```

Notice the `Author` and `Signed-off-by` lines match. If they don't your PR will be rejected by the
automated DCO check.

- In case you forgot to add it to the most recent commit, use `git commit --amend --signoff`
- In case you forgot to add it to the last N commits in your branch, use `git rebase --signoff HEAD~N` and replace N with the number of new commits you created in your branch.
- If you have already pushed your branch to a remote, will need to push your changes to overwrite the branch: `git push --force-with-lease origin my-branch`

## Support Channels

Whether you are a user or contributor, official support channels include:

- The GitHub Issues in each subproject repository
- [Slack](https://slack.wasmcloud.com/)
  - Please note that Slack is meant for help in discussing specific problems or asyncronous
    debugging/help. If you are reporting a specific bug, please do so it in GitHub Issues

Before opening a new issue or submitting a new pull request, it's helpful to search the project -
it's likely that another user has already reported the issue you're facing, or it's a known issue
that we're already aware of. It is also worth asking on the Slack channels.

<!-- NOTE: As the project matures, we may want to add a section about creating project milestones in GH that we adhere to -->

## Semantic Versioning

If you are not familiar with SemVer (the standard), [give it a quick read](https://semver.org/). We
follow SemVer with a high degree of rigor. 

As a pre-1.0 project, we are likely to have breaking changes on each release. However, these changes
must be _clearly documented_ in the release notes for each release. Once we release a 1.0 version of
wasmCloud, we will maintain a strong commitment to backward compatibility. All of our changes to
protocols and formats will be backward compatible from one minor release to the next. No features,
flags, commands, or APIs will be removed or substantially modified without a major version release
(unless absolutely needed to fix a security issue). This often means that we have to tell people
"no" or "wait" in order to preserve backward compatibility.

## Issues

Issues are used as the primary method for tracking anything to do with a wasmCloud project.

### Issue Types

There are 5 types of issues (each with their own corresponding [label](#labels)) in any wasmCloud
project:

- `question`: These are support or functionality inquiries that we want to have a record of for
  future reference. Generally these are questions that are too complex or large to store in the
  Slack channel or have particular interest to the community as a whole. Depending on the
  discussion, these can turn into `feature` or `bug` issues.
- `enhancement`: These track specific feature requests and ideas until they are complete. They can
  evolve from an [ADR](#proposing-an-idea) or can be submitted individually depending on the size.
- `bug`: These track bugs with the code
- `documentation`: These track problems with the documentation (i.e. missing or incomplete)

### Issue Lifecycle

The issue lifecycle is mainly driven by the project and org maintainers, but is good information for
those contributing to wasmCloud projects. All issue types follow the same general lifecycle.
Differences are noted below.

1. Issue creation
2. Triage
    - A maintainer or contributor will apply the proper labels for the issue. This includes labels
      for priority, type, and metadata (such as `good first issue`). The only issue priority we will
      be tracking is whether or not the issue is "critical." If additional levels are needed in the
      future, we will add them.
    - (If needed) Clean up the title to succinctly and clearly state the issue.
3. Discussion
    - Issues that are labeled `enhancement` must write an Architectural Decision Record (ADR) (see
      [Proposing an Idea](#proposing-an-idea)). Smaller quality-of-life enhancements are exempt. The
      decision about which enhancements are exempt are left to the decision of project maintainers
    - Issues that are labeled as `enhancement` or `bug` should be connected to the PR that resolves
      it once it is opened (see [How to Contribute Code](#how-to-contribute-code)).
    - Whoever is working on an `enhancement` or `bug` issue (whether a maintainer or someone from
      the community), should either assign the issue to themself or make a comment in the issue
      saying that they are taking it.
    - `question` issues should stay open until resolved or if they have not been active for more
      than 30 days. This will help keep the issue queue to a manageable size and reduce noise.
      Should the issue need to stay open, the `keep open` label can be added.
4. Issue closure/resolution

## Proposing an Idea

Before proposing a new idea to a wasmCloud project, please make sure to write up an [Architectural
Decision Record](https://wasmcloud.github.io/adr/). An Architectural Decision Record is a design
document that describes a new feature for a wasmCloud project. The proposal should provide a concise
technical specification and rationale for the feature. 

It is also worth considering vetting your idea with the community via Slack. Vetting an idea
publicly before going as far as writing a proposal is meant to save the potential author time.

ADRs are submitted to the [wasmcloud/adr repository](https://github.com/wasmCloud/adr/tree/gh-pages)
(submitted against the `gh-pages` branch). See
[ADR0000](https://wasmcloud.github.io/adr/0000-use-markdown-architectural-decision-records.html) for
a the specific structure chosen and the [provided
template](https://wasmcloud.github.io/adr/template.html) to write your own

After your proposal has been approved, you can go ahead and get started implementing it!

## Labels

The following tables define all label types used for wasmCloud projects. This does not preclude
individual projects from having additional labels, but all wasmCloud projects will have the same
base labels. The labels below are split up by category

### Common

| Label           | Description                                                                                                                            |
| --------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `bug`           | Marks an issue as a bug or a PR as a bugfix                                                                                            |
| `critical`      | Marks an issue or PR as critical. This means that addressing the PR or issue is top priority and must be addressed as soon as possible |
| `documentation` | Indicates the issue or PR is a documentation change                                                                                    |
| `enhancement`   | Marks the issue as a feature request or a PR as a feature implementation                                                               |
| `keep open`     | Denotes that the issue or PR should be kept open past 30 days of inactivity                                                            |
| `refactor`      | Indicates that the issue is a code refactor and is not fixing a bug or adding additional functionality                                 |

### Issue Specific

| Label              | Description                                                                                     |
| ------------------ | ----------------------------------------------------------------------------------------------- |
| `help wanted`      | Marks an issue needs help from the community to solve                                           |
| `proposal`         | Marks an issue as a proposal                                                                    |
| `question`         | Marks an issue as a support request or question                                                 |
| `good first issue` | Marks an issue as a good starter issue for someone new to the project                           |
| `wontfix`          | Marks an issue as discussed and will not be implemented (or accepted in the case of a proposal) |

### PR Specific

| Label      | Description                                               |
| ---------- | --------------------------------------------------------- |
| `breaking` | Indicates a PR has breaking changes (such as API changes) |

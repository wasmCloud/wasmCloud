# wasmCloud Governance

The following document outlines how the wasmCloud project governance operates.

- [wasmCloud Governance](#wasmcloud-governance)
  - [The wasmCloud Project](#the-wasmcloud-project)
  - [Maintainers Structure](#maintainers-structure)
    - [wasmCloud Org Maintainers](#wasmcloud-org-maintainers)
    - [New Maintainer Onboarding](#new-maintainer-onboarding)
    - [Stepping down as a maintainer](#stepping-down-as-a-maintainer)
    - [Taking a leave from being a maintainer](#taking-a-leave-from-being-a-maintainer)
  - [Decision Making at the wasmCloud org level](#decision-making-at-the-wasmcloud-org-level)
  - [Decision Making at the wasmCloud project level](#decision-making-at-the-wasmcloud-project-level)
  - [Communications](#communications)
  - [Code of Conduct](#code-of-conduct)
  - [DCO and Licenses](#dco-and-licenses)
  - [Pull Requests and Reviews](#pull-requests-and-reviews)

## The wasmCloud Project

The wasmCloud project is made up of several codebases and services with different release cycles. A
list of these projects can be found [here](https://github.com/orgs/wasmCloud/repositories).

## Maintainers Structure

There are two levels of maintainers for wasmCloud.
- **Org maintainers** oversee the overall project and its health.
- **Project maintainers** focus on a single codebase, a group of related codebases, a service (e.g., a website), or a supporting area (e.g., CI or community management).

See the [Contributor Ladder](./CONTRIBUTION_LADDER.md) for more detail on responsibilities and how to
progress through them.

### wasmCloud Org Maintainers

The wasmCloud Org maintainers are responsible for:

- Maintaining the mission, vision, values, and scope of the project
- Refining the governance and charter as needed
- Making project-level decisions
- Resolving escalated project decisions when a sub-team is blocked
- Managing the wasmCloud brand
- Controlling access to wasmCloud assets such as source repositories, hosting, and project calendars
- Handling code of conduct violations
- Deciding what sub-groups are part of the wasmCloud project
- Overseeing the resolution and disclosure of security issues
- Managing financial decisions related to the project

Changes to org maintainers use the following rules:

- There will be between 2 and 9 people.
- Any project maintainer of any active (non-archived) wasmCloud organization project is eligible for
  a position as an org maintainer.
- An org maintainer may step down by emailing the org maintainers or contacting them via Slack.
- Org maintainers MUST remain active on the project. If they are unresponsive for > 3 months they
  will lose org maintainership unless a
  [super-majority](https://en.wikipedia.org/wiki/Supermajority#Two-thirds_vote) of the other org
  maintainers agrees to extend the period.
- When there is an opening for a new org maintainer, any person who has made a contribution to any
  repo under the wasmCloud GitHub org may nominate a suitable project maintainer of an active
  project by contacting the current org maintainers.
  - The nomination period will be three weeks starting the day after an org maintainer opening
    becomes available.
- Org maintainers must vote for a nominated candidate; the vote must pass with a
  [super-majority](https://en.wikipedia.org/wiki/Supermajority#Two-thirds_vote).
- When an org maintainer steps down, they become an emeritus maintainer.

Once an org maintainer is elected, they remain a maintainer until stepping down (or, in rare cases,
are removed). Any existing project maintainer is eligible to become an org maintainer.

### New Maintainer Onboarding

When a new **project maintainer** is added, a current maintainer should take the following steps:

1. Open a PR adding the new maintainer to [MAINTAINERS.md](./MAINTAINERS.md) under the appropriate
   sub-team(s). The PR description should briefly describe the contributor's history and
   qualifications.
2. Merge the PR once approved (lazy consensus among existing project maintainers — no objection
   within 7 days is approval).
3. Add the new maintainer to the appropriate GitHub team(s) for their project area.
4. Announce the new maintainer in the wasmCloud Slack (`#wasmcloud-dev` or `#general`) and at the
   next community call.

When adding a new **org maintainer**, the following additional steps are required after the
above:

5. Add the new org maintainer to the CNCF
   [project-maintainers.csv](https://github.com/cncf/foundation/blob/main/project-maintainers.csv)
   via PR.
6. After the PR is merged, send an email to `cncf-maintainer-changes@cncf.io` noting the addition
   so they can be added to the `cncf-wasmCloud-maintainers@lists.cncf.io` mailing list.

### Stepping down as a maintainer

To step down, a maintainer should open a PR to remove themselves (and add themselves to the
Emeritus section) from [MAINTAINERS.md](./MAINTAINERS.md). Upon merge:

1. Another maintainer removes the departing maintainer from their GitHub team(s).
2. (Encouraged) A thank-you message is sent in Slack and at the next community call.

If an **org maintainer** steps down, the additional steps are:

3. Remove them from the CNCF
   [project-maintainers.csv](https://github.com/cncf/foundation/blob/main/project-maintainers.csv)
   via PR.
4. Send an email to `cncf-maintainer-changes@cncf.io` noting the removal.

### Taking a leave from being a maintainer

Any maintainer may take a leave of absence for any reason (which they are not required to
disclose). To do so, open a PR to mark yourself as "on leave" in [MAINTAINERS.md](./MAINTAINERS.md).
project maintainers. Upon returning, open a PR to remove the "on leave" designation — no additional vote required.

If a maintainer on leave has not contacted other maintainers after 6 months, they may be moved to emeritus status.
designation — no additional vote required.

## Decision Making at the wasmCloud org level

The default decision-making process is
[lazy consensus](http://communitymgt.wikia.com/wiki/Lazy_consensus). Any decision is considered
supported as long as no one objects. Silence is implicit agreement.

When consensus cannot be found a maintainer can call for a
[majority](https://en.wikipedia.org/wiki/Majority) vote.

The following decisions **must** be put to a vote:

- Enforcing a CoC violation (super-majority)
- Removing a maintainer for any reason other than inactivity (super-majority)
- Changing this governance document (super-majority)
- Licensing and intellectual property changes, including new logos or wordmarks (simple majority)
- Adding, archiving, or removing sub-projects (simple majority)
- Utilizing wasmCloud/CNCF funds for anything CNCF deems "not cheap and easy" (simple majority)

Other decisions may be called to a vote at any time by any maintainer; by default such votes
require a _simple majority_.

## Decision Making at the wasmCloud project level

Project maintainers are free to set their own decision-making processes. The default is
[lazy consensus](http://communitymgt.wikia.com/wiki/Lazy_consensus).

Day-to-day decisions (e.g., agreeing on ADRs, release timing) can be made by a
[simple majority](https://en.wikipedia.org/wiki/Majority). When your maintainer group spans multiple
organizations, aim to include at least one vote from each represented organization.

## Communications

The two primary communication channels for the wasmCloud project are:

- GitHub Issues / PRs / Discussions
- [wasmCloud Slack](https://slack.wasmcloud.com)

To reach the org maintainers, use the wasmCloud Slack or email
`cncf-wasmCloud-maintainers@lists.cncf.io`.

## Code of Conduct

This project follows the [CNCF Code of
Conduct](https://github.com/cncf/foundation/blob/main/code-of-conduct.md). Possible violations
should be reported to the org maintainers.

If the possible violation involves an org maintainer, that maintainer will be recused from the
decision. Such issues must be escalated to the appropriate CNCF contact, and CNCF may choose to
intervene.

## DCO and Licenses

The following licenses and contributor agreements apply to wasmCloud projects:

- [Apache 2.0](https://opensource.org/licenses/Apache-2.0) for code
- [Creative Commons Attribution 4.0 International Public
  License](https://creativecommons.org/licenses/by/4.0/legalcode) for documentation
- [Developer Certificate of Origin](https://developercertificate.org/) for new contributions

## Pull Requests and Reviews

Pull requests are reviewed by maintainers and community members. Maintainers are responsible for
triaging PRs and ensuring they receive appropriate review. Each pull request must pass the
checks defined for its project area. Project-level required checks are decided at the project level.

Additional contributing guidelines can be found in [CONTRIBUTING.md](./CONTRIBUTING.md).

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
  - [Code of Conduct](#code-of-conduct)
  - [DCO and Licenses](#dco-and-licenses)
  - [Pull Requests and Reviews](#pull-requests-and-reviews)

## The wasmCloud Project

The wasmCloud project is made up of several codebases and services with different release cycles. A
list of these projects can be found [here](https://github.com/orgs/wasmCloud/repositories).

## Maintainers Structure

There are two levels of maintainers for wasmCloud. The wasmCloud org maintainers oversee the overall
project and its health. Project maintainers focus on a single codebase, a group of related
codebases, a service (e.g., a website), or project to support the other projects (e.g., marketing or
community management). For example, the provider maintainers manage the capability providers
repository. See the [Contributor Ladder](./CONTRIBUTION_LADDER.md) for more detailed information on
responsibilities.

<!-- TODO: We should choose a canonical communication mechanism (like a mailing list) an require -->

### wasmCloud Org Maintainers

The wasmCloud Org maintainers are responsible for:

- Maintaining the mission, vision, values, and scope of the project
- Refining the governance and charter as needed
- Making project level decisions
- Resolving escalated project decisions when the subteam responsible is blocked
- Managing the wasmCloud brand
- Controlling access to wasmCloud assets such as source repositories, hosting, project calendars
- Handling code of conduct violations
- Deciding what sub-groups are part of the wasmCloud project
- Overseeing the resolution and disclosure of security issues
- Managing financial decisions related to the project

Changes to org maintainers use the following:

- There will be between 3 and 9 people.
- Any project maintainer of any active (non-archived) wasmCloud organization project is eligible for
  a position as an org maintainer.
- An org maintainer may step down by emailing the org maintainers or contacting them through Slack
- Org maintainers MUST remain active on the project. If they are unresponsive for > 3 months they
  will lose org maintainership unless a
  [super-majority](https://en.wikipedia.org/wiki/Supermajority#Two-thirds_vote) of the other org
  maintainers agrees to extend the period to be greater than 3 months
- When there is an opening for a new org maintainer, any person who has made a contribution to any
  repo under the wasmCloud GitHub org may nominate a suitable project maintainer of an active
  project as a replacement by contacting the current org maintainers
  - The nomination period will be three weeks starting the day after an org maintainer opening
    becomes available
- Org maintainers must vote for a nominated maintainer and the vote must pass with a
  [super-majority](https://en.wikipedia.org/wiki/Supermajority#Two-thirds_vote).
- When an org maintainer steps down, they become an emeritus maintainer

Once an org maintainer is elected, they remain a maintainer until stepping down (or, in rare cases,
are removed). Voting for new maintainers occurs when necessary to fill vacancies. Any existing
project maintainer is eligible to become an org maintainer.

### New Maintainer Onboarding

When a new maintainer is added, a current maintainer should take the following steps:

- Add the new maintainer to the [MAINTAINERS.md](./MAINTAINERS.md) file in this repo or the
  appropriate subproject repo
- Add the new maintainer to the appropriate GitHub group for their project
- Announce the new maintainer to the wasmCloud community via the wasmCloud Slack and during the
  community call

When adding a new org maintainer, a few additional steps are required:

- Add the new org maintainer to the CNCF
  [project-maintainers.csv](https://github.com/cncf/foundation/blob/main/project-maintainers.csv)
  file via PR
- Once the PR is merged, send an email to cncf-maintainer-changes@cncf.io noting that the new
  maintainer has been added and that they should be added to the
  cncf-wasmCloud-maintainers@lists.cncf.io list

### Stepping down as a maintainer

To step down as a maintainer, that maintainer should open a PR to remove themselves (and add
themselves to the emeritus section) from the MAINTAINERS.md file in this repo or the appropriate
subproject repo, declaring their intent to step down.

Upon the PR being merged, another maintainer should take the following steps:

- Remove the maintainer from the appropriate GitHub group for their project
- (Optional, but suggested) Send a thank you message and announcement via the wasmCloud Slack and
  during the community call

If an org maintainer steps down, the following steps should be taken in addition to the above:

- Remove the maintainer from the CNCF
  [project-maintainers.csv](https://github.com/cncf/foundation/blob/main/project-maintainers.csv)
  file via PR
- Send an email to cncf-maintainer-changes@cncf.io noting that the maintainer has been removed and
  that they should be removed from the cncf-wasmCloud-maintainers@lists.cncf.io list

### Taking a leave from being a maintainer

There are many reasons why an active maintainer may need to take a break. Any maintainer of a
wasmCloud project (including org maintainers) are welcome to take leave from being a maintainer, for
any reason (which they are not required to disclose). At the end of the leave, they can resume their
duties as a maintainer with no additional votes or governance. The maximum amount of leave is 6
months (2 times the maximum amount of time a maintainer can be absent before being considered
inactive). A leave can be extended for an additional 6 months beyond that based on a majority vote
by the other project maintainers of the project to which they belong (i.e. if someone maintains the
host, the host maintainers would make the decision). These decisions can be overriden by a
super-majority of the org maintainers. If a maintainer is marked as being on leave and has not
contacted other maintainers after 6 months, they will be moved to emeritus status in the same way as
if they had been inactive.

To take a break from being a maintainer, the maintainer should open a PR to mark themselves as "on
leave" (in parenthesis next to their name) in the appropriate MAINTAINERS.md file or ask a fellow
maintainer to do so if circumstances prevent them from doing it themselves. If possible (but not
required), the maintainer should give an approximate length of leave in the PR. If the time is not
specified, the "expected" length defaults to 6 months. 

Upon returning from leave, the maintainer should open a PR to remove the "on leave" designation.

## Decision Making at the wasmCloud org level

When maintainers need to make decisions there are two ways decisions are made, unless described
elsewhere.

The default decision making process is
[lazy-consensus](http://communitymgt.wikia.com/wiki/Lazy_consensus). This means that any decision is
considered supported by the team making it as long as no one objects. Silence on any consensus
decision is implicit agreement and equivalent to explicit agreement. Explicit agreement may be
stated at will.

When a consensus cannot be found a maintainer can call for a
[majority](https://en.wikipedia.org/wiki/Majority) vote on a decision.

Many of the day-to-day project maintenance can be done by a lazy consensus model. But the following
items must be called to vote:

- Enforcing a CoC violation (super majority)
- Removing a maintainer for any reason other than inactivity (super majority)
- Changing the governance rules (this document) (super majority)
- Licensing and intellectual property changes (including new logos, wordmarks) (simple majority)
- Adding, archiving, or removing subprojects (simple majority)
- Utilizing wasmCloud/CNCF money for anything CNCF deems "not cheap and easy" (simple majority)

Other decisions may, but do not need to be, called out and put up for decision at any time and by
anyone. By default, any decisions called to a vote will be for a _simple majority_ vote.

## Decision Making at the wasmCloud project level

Project maintainers are free to set their own decision making processes in most cases. As with org
level decisions, the default decision making process is
[lazy-consensus](http://communitymgt.wikia.com/wiki/Lazy_consensus). This means that any decision is
considered supported by the team making it as long as no one objects. Silence on any consensus
decision is implicit agreement and equivalent to explicit agreement. Explicit agreement may be
stated at will.

Decisions pertaining to day-to-day operations (such as agreeing on ADRs or when to release) can be
done with a [simple majority](https://en.wikipedia.org/wiki/Majority) vote. However, it is
recommended to make sure and have a vote from all interested parties. For example, if you have 2
maintainers from company X and 1 from company Y, you should have at least 1 vote from both company X
and company Y

## Code of Conduct

This project follows the [CNCF Code of
Conduct](https://github.com/cncf/foundation/blob/main/code-of-conduct.md). Possible code of conduct
violations should be sent to the org maintainers.

If the possible violation is against one of the org maintainers that member will be recused from
voting on the issue. Such issues must be escalated to the appropriate CNCF contact, and CNCF may
choose to intervene.

## DCO and Licenses

The following licenses and contributor agreements will be used for wasmCloud projects:

- [Apache 2.0](https://opensource.org/licenses/Apache-2.0) for code
- [Creative Commons Attribution 4.0 International Public
  License](https://creativecommons.org/licenses/by/4.0/legalcode) for documentation
- [Developer Certificate of Origin](https://developercertificate.org/) for new contributions

## Pull Requests and Reviews

Pull requests are reviewed by maintainers and community members. Maintainers are responsible for
triaging pull requests and ensuring that they are reviewed. Maintainers are also responsible for
ensuring that the pull request is reviewed by the appropriate people. For example, a pull request
that changes the website should be reviewed by the website maintainers. Each pull request
in the wasmCloud organization will be required to pass appropriate checks, and these checks
are decided at the project level.

Additional information about contributing guidelines can be found in [CONTRIBUTING.md](./CONTRIBUTING.md).

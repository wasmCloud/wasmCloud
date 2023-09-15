# wasmCloud Governance

The following document outlines how the wasmCloud project governance operates.

- [The wasmCloud Project](#the-wasmcloud-project)
- [Maintainers Structure](#maintainers-structure)
  - [wasmCloud Org Maintainers](#wasmcloud-org-maintainers)
- [Decision Making at the wasmCloud org level](#decision-making-at-the-wasmcloud-org-level)
- [Decision Making at the wasmCloud project level](#decision-making-at-the-wasmcloud-project-level)
- [Code of Conduct](#code-of-conduct)
- [DCO and Licenses](#dco-and-licenses)
- [Pull Requests and Reviews](#pull-requests)

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

The code of conduct is overseen by the wasmCloud org maintainers. Possible code of conduct
violations should be sent to the org maintainers <!-- TODO: Update this to a mailing list once we
have one  -->

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

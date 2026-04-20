# wasmCloud Security Process and Policy

This document provides details on the wasmCloud security policy and the processes surrounding
security handling, including how to report a vulnerability for anything within the wasmCloud
organization.

- [Report A Vulnerability](#report-a-vulnerability)
  - [When To Send A Report](#when-to-send-a-report)
  - [When Not To Send A Report](#when-not-to-send-a-report)
  - [Security Vulnerability Response](#security-vulnerability-response)
  - [Public Disclosure](#public-disclosure)
- [Security Team Membership](#security-team-membership)
  - [Responsibilities](#responsibilities)
  - [Membership](#membership)
- [Patch and Release Team](#patch-and-release-team)
- [Disclosures](#disclosures)

## Report A Vulnerability

We are extremely grateful for security researchers and users who report vulnerabilities to the
wasmCloud community. All reports are thoroughly investigated by a set of wasmCloud maintainers.

To make a report, email the private security list at **security@wasmcloud.com** with the details.
You can also use [GitHub's private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing/privately-reporting-a-security-vulnerability)
on any repository in the wasmCloud GitHub organization.

Reports are received by the wasmCloud security team, which is a subset of active org maintainers.
See [Security Team Membership](#security-team-membership) for details.

### When To Send A Report

You believe you have found a vulnerability in a wasmCloud project or in a dependency of a wasmCloud
project. This includes any repository in the [wasmCloud GitHub
organization](https://github.com/wasmCloud).

### When Not To Send A Report

- A vulnerability found in an **application deployed to** a wasmCloud host (report to that
  application's maintainers instead).
- You are looking for guidance on securing a wasmCloud deployment — see the
  [documentation](https://wasmcloud.com/docs) or ask in [Slack](https://slack.wasmcloud.com).
- You are looking for help applying security updates.

### Security Vulnerability Response

Each report will be reviewed and receipt acknowledged within **3 business days**. This begins the
security review process described below.

Vulnerability information shared with the security team stays within the wasmCloud project and will
not be shared with others unless it is necessary to fix the issue. Information is shared only on a
need-to-know basis.

We ask that vulnerability reporters act in good faith by not disclosing the issue to others. We
strive to act in good faith by responding swiftly and by crediting reporters in writing (with their
permission).

As the security issue moves through triage, identification, and release, the reporter will be
notified. We may ask additional questions of the reporter during this process.

### Public Disclosure

A public disclosure of security vulnerabilities is released alongside the release that fixes the
vulnerability. We aim to fully disclose vulnerabilities once a mitigation strategy is available.
Our goal is to perform a release and public disclosure quickly and in a timetable that works well
for users.

CVEs will be assigned to vulnerabilities. Because obtaining a CVE ID takes time, a disclosure may
be published before the CVE ID is assigned; the disclosure will be updated once the ID is available.

If the vulnerability reporter would like to be credited as part of the disclosure, we are happy to
do so. We will ask for permission and for how the reporter would like to be identified.

## Security Team Membership

The security team is a subset of active wasmCloud project maintainers who are willing and able to
respond to vulnerability reports.

### Responsibilities

- Members **MUST** be active project maintainers on active (non-deprecated) wasmCloud projects.
- Members **SHOULD** engage in each reported vulnerability, at a minimum to ensure it is being
  handled.
- Members **MUST** keep vulnerability details private and share only on a need-to-know basis.

### Membership

New members must be active wasmCloud project maintainers willing to fulfill the responsibilities
above. Members can step down at any time and may join at any time by contacting the org maintainers.

If a security team member is no longer an active maintainer on any active wasmCloud project, they
will be removed from the team.

## Patch and Release Team

When a vulnerability is acknowledged, a team — including maintainers of the affected wasmCloud
project — will be assembled to patch the vulnerability, release an update, and publish the
disclosure. This team may expand beyond the security team as needed but will remain within the pool
of active wasmCloud project maintainers.

## Disclosures

Vulnerability disclosures are published as GitHub Security Advisories in the relevant project
repository. Each disclosure will contain:

- An overview of the vulnerability
- Details about the affected component and versions
- A fix (typically a new release)
- A workaround, if one is available

Disclosures are published on the same day as the fixing release, after the release is published.
Release notes will contain a link to the advisory.

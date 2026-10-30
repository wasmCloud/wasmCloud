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
- [Supported Versions](#supported-versions)
- [Dependency and Code Scanning](#dependency-and-code-scanning)
- [Secrets and Credentials](#secrets-and-credentials)

## Report A Vulnerability

CRA stewardship: This project is supported under the Linux Foundation CRA stewardship framework, as described at https://www.linuxfoundation.org/security. Security vulnerabilities should be reported through the mechanisms described below, which we will coordinate with our CRA steward. For actively exploited vulnerabilities and severe incidents that may require CRA escalation, please use the project's emergency security reporting mechanisms as appropriate.

We are extremely grateful for security researchers and users who report vulnerabilities to the
wasmCloud community. All reports are thoroughly investigated by a set of wasmCloud maintainers.

To make a report, email the private security list at **security@wasmcloud.com** with the details.
This works for any repository in the wasmCloud GitHub organization.

You can also use [GitHub's private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing/privately-reporting-a-security-vulnerability)
on these repositories:

- [wasmCloud/wasmCloud](https://github.com/wasmCloud/wasmCloud/security/advisories/new)
- [wasmCloud/go](https://github.com/wasmCloud/go/security/advisories/new)
- [wasmCloud/typescript](https://github.com/wasmCloud/typescript/security/advisories/new)
- [wasmCloud/wasmcloud.com](https://github.com/wasmCloud/wasmcloud.com/security/advisories/new)
- [wasmCloud/awesome-wasmcloud](https://github.com/wasmCloud/awesome-wasmcloud/security/advisories/new)

Reports are received by the wasmCloud security team, which is a subset of active org maintainers.
See [Security Team Membership](#security-team-membership) for details.

### When To Send A Report

You believe you have found a vulnerability in a wasmCloud project or in a dependency of a wasmCloud
project. This includes any repository in the [wasmCloud GitHub
organization](https://github.com/wasmCloud). For what this repository treats as a vulnerability,
and what it does not, see the [threat model](THREAT_MODEL.md).

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
not be shared with others unless it is necessary to fix the issue or to [escalate it to the CRA
steward](CONTRIBUTING.md#escalating-to-the-cra-steward). Information is shared only on a
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

## Supported Versions

Security fixes ship only in the latest minor release. When a new minor release ships, the previous
one stops receiving security fixes. wasmCloud 1.x is end of life. See [SUPPORT.md](SUPPORT.md) for
the full support policy.

## Dependency and Code Scanning

This policy covers released code: `wash`, `wash-runtime`, `runtime-operator`, `runtime-gateway`
and the `runtime-operator` Helm chart. Examples and templates receive Dependabot updates, but
their findings don't block a release.

Findings come from:

- [`cargo audit`](https://rustsec.org) and `cargo deny check licenses`, which run in
  [`audit.yml`](.github/workflows/audit.yml) daily, on pushes to `main`, and on pull requests that
  change dependencies.
- Dependabot alerts for Cargo, Go modules and GitHub Actions.
- CodeQL and zizmor code scanning, reported in the repository's Security tab.

### Vulnerabilities

A vulnerability that affects released code is fixed on this timeline, by its advisory's severity:

| Severity | Fixed in |
| -------- | -------- |
| Critical | An out-of-cycle patch release within 7 days of a fix being available |
| High     | The next scheduled release, at most 14 days |
| Medium   | A release within 90 days |
| Low      | Best effort |

An advisory that doesn't affect wasmCloud is suppressed in [`.cargo/audit.toml`](.cargo/audit.toml)
with a comment saying why it doesn't apply.

### Licenses

Every dependency must use a license on the allowlist in [`deny.toml`](deny.toml), which mirrors the
[CNCF allowed license policy](https://github.com/cncf/foundation/blob/main/policies-guidance/allowed-third-party-license-policy.md).
A dependency under any other license can't merge without a per-crate exception in `deny.toml` that
links its [CNCF exception request](https://exceptions.cncf.io/). If CNCF declines the request, the
dependency is replaced.

### Code scanning

Code scanning alerts in released code follow the vulnerability timeline above, by the alert's
security severity. Each alert is either fixed or dismissed in the Security tab with a stated
reason, such as code that only runs in tests.

### Before a release

A release PR can't merge while `cargo audit` or `cargo deny check licenses` fails, or while a
known critical or high advisory that affects released code is open.

## Secrets and Credentials

This policy covers the credentials wasmCloud's CI and release automation use.

### Prefer no stored secret

Use a short-lived credential whenever the service supports one:

- Signing and provenance use Sigstore keyless signing with the workflow's GitHub OIDC identity.
- Cloud access uses OIDC role assumption. The benchmark workflows assume an AWS role this way.
- GitHub API access beyond the current workflow's token uses a short-lived token from
  [octo-sts](https://github.com/octo-sts/app) or a GitHub App, not a personal access token.
- Everything else uses the workflow's `GITHUB_TOKEN`. Each workflow sets its own `permissions:`,
  and the repository default is read-only.

### Stored secrets

When a service offers none of these, the credential is stored as a GitHub Actions secret:

- Secrets never go in the repository, container images, build artifacts or logs. Secret scanning
  and push protection are on for wasmCloud's repositories.
- Each secret has the narrowest scope that works: one repository, or a deployment environment
  restricted to `main` and release tags for secrets that publish or release. An organization
  secret is shared only with the repositories that use it.
- Only org maintainers and repository admins can create, update or delete secrets. Anyone with
  write access can run a workflow that reads repository secrets, so write access follows
  [Granting Access](CONTRIBUTION_LADDER.md#granting-access).
- Org maintainers keep a private record of each secret: what it is for, who owns it, and when it
  was last rotated. A secret no workflow uses is deleted.

### Rotation

A secret is rotated:

- Immediately, if it may have been exposed.
- When someone who held its value, or an org maintainer or repository admin, loses that access.
- At least once a year, for any credential that doesn't expire on its own.

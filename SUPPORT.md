# Support

## Supported releases

wasmCloud supports only its latest minor release. Today that is 2.11.

- Bug fixes and security fixes land on `main` and ship in the next release. The
  [release train](RELEASE_RUNBOOK.md#release-cadence) cuts a release every two weeks, so a fix
  waits at most two weeks.
- A fix for a critical vulnerability ships as an out-of-cycle patch release as soon as it is ready.
- When a new minor release ships, the previous minor release stops receiving any fixes, including
  security fixes. For example, 2.11 stops receiving fixes when 2.12.0 is released.
- Fixes are not backported. To get a fix, upgrade to the latest release.

`wash`, `wash-runtime`, `runtime-operator`, `runtime-gateway` and the `runtime-operator` Helm chart
are released together under one `vX.Y.Z` version, so they share this support window.

## wasmCloud 1.x

wasmCloud 1.x is end of life and receives no fixes, including security fixes. Its last release was
v1.9.2, on February 4, 2026. Upgrade to the latest 2.x release.

## Getting help

- Ask questions in [Slack](https://slack.wasmcloud.com/) or
  [GitHub Discussions](https://github.com/wasmCloud/wasmCloud/discussions), or bring them to a
  [community meeting](https://wasmcloud.com/community/).
- Report bugs as [GitHub issues](https://github.com/wasmCloud/wasmCloud/issues/new/choose). See
  [CONTRIBUTING.md](CONTRIBUTING.md#reporting-bugs) for what to include.
- Report security vulnerabilities privately, as described in [SECURITY.md](SECURITY.md).

Support is provided by the community on a best-effort basis.

# Release Runbook

This guide is a runbook for maintainers preparing to release components of this monorepo.

wasmCloud v2 uses **tag-triggered automation** for all releases. There is no manual workflow
dispatch or smart-release tooling. Pushing a correctly formatted tag to `main` kicks off the
appropriate CI pipeline automatically.

## Release artifacts at a glance

| Component | Tag format | What gets published |
|-----------|-----------|---------------------|
| `wash` + `runtime-gateway` + `runtime-operator` + Helm charts | `vX.Y.Z` | Binaries, Docker images, Helm chart, GitHub Release |
| `wash-runtime` (library crate) | `wash-runtime-vX.Y.Z` | crates.io |

> **Note:** A single `vX.Y.Z` tag simultaneously triggers releases for `wash`, `runtime-gateway`,
> `runtime-operator`, and the Helm chart. These components are co-versioned.

## Canary builds

On every merge to `main` (no tag required):

- `ghcr.io/wasmcloud/wash:canary-v2`
- `ghcr.io/wasmcloud/runtime-gateway:canary`
- `ghcr.io/wasmcloud/runtime-operator:canary`
- Helm chart `runtime-operator:v2-canary`

## Releasing `wash` / `runtime-gateway` / `runtime-operator` / Helm charts

These four components share a version tag and are released together.

1. Update the version in `Cargo.toml` (root workspace) to `X.Y.Z`:
   ```
   version = "X.Y.Z"
   ```
2. Open a pull request with only this version bump, get it reviewed, and merge to `main`.
3. Create and push the release tag from `main`:
   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
4. The `wash` workflow will:
   - Build cross-platform binaries (Linux x86\_64/aarch64 musl, macOS x86\_64/aarch64, Windows x86\_64)
   - Generate SLSA build provenance attestations
   - Create a GitHub Release with binaries attached and auto-generated release notes
   - Push `ghcr.io/wasmcloud/wash:vX.Y.Z`
   - Automatically dispatch a Homebrew tap update
5. The `runtime-gateway` workflow will push `ghcr.io/wasmcloud/runtime-gateway:vX.Y.Z`.
6. The `runtime-operator` workflow will:
   - Push `ghcr.io/wasmcloud/runtime-operator:vX.Y.Z`
   - Create a Go module tag `runtime-operator/vX.Y.Z`
7. The `charts` workflow will publish the `runtime-operator` Helm chart to
   `ghcr.io/wasmcloud/charts/runtime-operator:vX.Y.Z`.

Monitor the [Actions tab](https://github.com/wasmCloud/wasmCloud/actions) to confirm all workflows
complete successfully.

### Pre-release (e.g., release candidate)

Tag with a pre-release suffix, e.g., `v2.1.0-rc.1`.

## Releasing `wash-runtime` (library crate)

`wash-runtime` is a library crate published independently to crates.io.

1. Update the version in `crates/wash-runtime/Cargo.toml` to `X.Y.Z`.
2. Open a pull request with the version bump, get it reviewed, and merge to `main`.
3. Create and push the release tag from `main`:
   ```bash
   git tag wash-runtime-vX.Y.Z
   git push origin wash-runtime-vX.Y.Z
   ```
4. The `wash-runtime` workflow will:
   - Verify the tag version matches the version in `Cargo.toml` (fails fast if mismatched)
   - Publish the crate to crates.io using the `CRATES_PUBLISH_TOKEN` secret

## Versioning policy

wasmCloud follows [Semantic Versioning](https://semver.org/):

- **Patch** (`X.Y.Z+1`): Bug fixes and backward-compatible changes.
- **Minor** (`X.Y+1.0`): New backward-compatible features.
- **Major** (`X+1.0.0`): Breaking changes. Coordinate with maintainers before bumping major.

Conventional commit prefixes used to categorize changes in release notes:

| Prefix | Type |
|--------|------|
| `fix:` | Bug fix |
| `feat:` | New feature |
| `feat!:` / `BREAKING CHANGE:` | Breaking change |
| `chore:`, `docs:`, `test:`, `refactor:` | Non-functional (no version bump implied) |

## Checklist before tagging

- [ ] All CI checks pass on `main`
- [ ] `Cargo.toml` (or crate-specific `Cargo.toml`) version is updated and merged
- [ ] CHANGELOG or release notes are accurate (auto-generated from conventional commits)
- [ ] No open regressions targeting this release
- [ ] For major releases: migration guide is documented

## Checklist after tagging

- [ ] All release workflows completed successfully in GitHub Actions
- [ ] GitHub Release is visible with correct binaries attached
- [ ] Docker images are available on GHCR
- [ ] Homebrew tap updated (check [homebrew-wasmcloud](https://github.com/wasmCloud/homebrew-wasmcloud))
- [ ] For `wash-runtime`: crate is visible on [crates.io](https://crates.io/crates/wash-runtime)
- [ ] Announce the release in [Slack](https://slack.wasmcloud.com) `#announcements`

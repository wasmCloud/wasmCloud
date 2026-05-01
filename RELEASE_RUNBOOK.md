# Release Runbook

This guide is a runbook for maintainers preparing to release components of this monorepo.

wasmCloud v2 uses **tag-triggered automation** for all releases. Pushing a correctly formatted
tag to `main` kicks off the appropriate CI pipeline automatically. The tag itself is normally
pushed by the [release train](#release-cadence); manual tag pushes are reserved for the rare
case the train is offline (see [If the train is offline](#if-the-train-is-offline)).

## Release cadence

wasmCloud follows a **train release model**: a release is cut every two weeks on **Tuesday at
16:00 UTC**. Tuesday avoids the Monday catch-up window and the Friday/pre-weekend cliff, leaving
Wed–Thu as a buffer for any same-week follow-up patch.

- **No skip conditions.** The train leaves on schedule, holidays and US weekends included. If
  something is broken, we revert and re-cut, not skip. Predictability is the whole point.
- **Patch releases out-of-cycle are allowed at any time** for critical fixes. The cadence is a
  floor, not a ceiling. Trigger an out-of-cycle release via `workflow_dispatch` on
  [`release-train.yml`](.github/workflows/release-train.yml).
- **Single co-versioned release.** `wash`, `wash-runtime`, `runtime-gateway`, `runtime-operator`,
  and the `runtime-operator` Helm chart all ship under a single `vX.Y.Z` tag.
- **Mostly-push-button automation.** The train opens the PR, runs CI, waits for canary builds on
  the merge commit to pass, then pushes the immutable tag. Today, a maintainer still needs to
  approve the release PR before auto-merge fires (see [Approval expectation](#approval-expectation)).

### How the train works

1. [`release-train.yml`](.github/workflows/release-train.yml) runs on a weekly Tuesday cron with
   an ISO-week-parity gate that enforces the every-two-weeks cadence. `workflow_dispatch`
   bypasses the gate for off-cycle runs and accepts `bump` (patch/minor/major) or an explicit
   `version` input.
2. The job opens a `release/vX.Y.Z` branch with a patch-bump (default). It edits the workspace
   `Cargo.toml`, refreshes `Cargo.lock` via `cargo update --workspace` (with a sanity check that
   only workspace member entries change), and bumps `appVersion` in
   `charts/runtime-operator/Chart.yaml`.
3. A PR labeled `release-train` is opened; the job enables **rebase auto-merge** (not squash —
   see [Why rebase merge](#why-rebase-merge)) and watches the PR's required checks. If checks
   fail, the job fails loudly with a stage-tagged Slack message to `#release-bot` so a human
   can fix forward the same day.
4. The job then waits for the PR to actually merge. Once merged, the merge commit's subject is
   exactly `release: vX.Y.Z` (rebase preserves the branch's commit subject; no `(#NNN)`
   PR ref is appended). That subject is what `wash.yml`'s `canary-binaries` job keys off to run
   the matrix release build, and what `release-tag.yml` matches to identify a release commit.
5. After the PR merges, [`release-tag.yml`](.github/workflows/release-tag.yml) fires via
   `workflow_run` and performs the **build-once-promote** flow:
   - Verifies HEAD on `main` is a release commit and the workspace version matches.
   - Waits for the canary workflows (`wash`, `runtime-operator`, `runtime-gateway`, `charts`) on
     the merge commit to all conclude `success` — the artifact pipeline has built and tested
     this exact commit.
   - **Promotes** each canary container image: retags
     `ghcr.io/wasmcloud/{wash,runtime-gateway,runtime-operator}:sha-<merge-sha>` as `vX.Y.Z`.
     Same manifest digest, no rebuild — the bytes that passed canary CI are the bytes users pull.
   - **Re-attests** each promoted manifest digest (Sigstore-signed via Fulcio).
   - **Downloads** the canary-binaries artifacts from the wash.yml run that built them, attests
     them, and (after pushing the tag) creates the GitHub Release with those exact binaries.
   - Pushes the annotated `vX.Y.Z` git tag at the validated merge commit. **The tag is
     immutable**: the workflow never force-pushes, and a re-run that finds the tag already on
     origin is a no-op.
6. After pushing `vX.Y.Z`, `release-tag.yml` also pushes the
   `runtime-operator/vX.Y.Z` Go module tag (so module consumers never see it before the
   matching OCI release exists), then creates the GitHub Release with the promoted binaries,
   then dispatches Homebrew and runs winget update inline. All sequenced after canary
   validation.
7. The tag push triggers the remaining tag-listening workflows: `wash-runtime` publishes the
   library crate to crates.io (idempotent on re-run — already-published versions exit 0);
   `charts.yml`'s release job repackages the chart at `vX.Y.Z` (charts are templated and
   deterministic, so rebuild is acceptable here).

### Why rebase merge

The release-train PR uses `gh pr merge --auto --rebase`, not `--squash`. With squash-merge,
GitHub appends ` (#NNN)` to the commit subject (e.g. `release: v2.0.6 (#1234)`), which
breaks the strict subject contract that `release-tag.yml` and `canary-binaries` rely on. With
rebase-merge, the branch's commit message is preserved verbatim — `release: v2.0.6` —
which is what those workflows match.

This requires "Allow rebase merging" enabled in repo settings (alongside "Allow auto-merge").

### Approval expectation

Today, branch protection on `main` requires a maintainer review on every PR, including
release-train PRs. The train will:

- Open the PR (auto)
- Wait for required CI to go green (auto)
- **Wait indefinitely (up to 2h timeout) for a maintainer's approval** — at which point
  auto-merge fires (auto from there)

Maintainers should treat release-train PRs as a normal review item: read the diff (only
`Cargo.toml`, `Cargo.lock`, and `Chart.yaml` should change), confirm the version bump matches
expectations, approve. If the PR sits unapproved past the 2h timeout, the train fails with a
Slack page; resolve and re-dispatch.

### Bumping minor or major

Patch is the default. To cut a minor or major release on the train, dispatch
`release-train.yml` manually with the `bump` input set to `minor` or `major`. Majo and minor bumps
should be coordinated with maintainers ahead of time (see [Versioning policy](#versioning-policy)).

### If the train is offline

If `release-train.yml` is broken or the bot can't push, do by hand what the train does. The
**commit subject must be exactly `release: vX.Y.Z`** — that is what triggers the
matrix binary build and is what `release-tag.yml` looks for to identify a release commit.

1. Bump the workspace version in `Cargo.toml` to `X.Y.Z` and `appVersion` in
   `charts/runtime-operator/Chart.yaml` to the same value. Run `cargo update --workspace`.
2. Commit with subject `release: vX.Y.Z`. Open a PR, get it reviewed, **rebase-merge**
   to `main` (rebase preserves the subject verbatim; squash-merge would append ` (#NNN)` and
   break `release-tag.yml`'s subject match).
3. Wait for canary builds on the merge commit to all succeed (`wash`, `runtime-operator`,
   `runtime-gateway`, `charts`, plus `canary-binaries` in `wash.yml`).
4. Re-run `release-tag.yml` from the Actions tab (**Re-run failed jobs** on the failed
   workflow_run instance) so it picks up the now-green canaries, or perform its work by hand:
   - Promote the canary OCI images:
     ```bash
     for img in ghcr.io/wasmcloud/{wash,runtime-gateway,runtime-operator}; do
       docker buildx imagetools create --tag "${img}:vX.Y.Z" "${img}:sha-<MERGE_SHA>"
     done
     ```
   - Push the immutable annotated tag and the Go module tag:
     ```bash
     git tag -a vX.Y.Z <MERGE_SHA> -m "Release vX.Y.Z"
     git tag -a runtime-operator/vX.Y.Z <MERGE_SHA> -m "Release runtime-operator/vX.Y.Z"
     git push origin vX.Y.Z runtime-operator/vX.Y.Z
     ```
   - Download the canary-binaries from the merge commit's `wash.yml` run and create the
     GitHub Release with them attached
     (`gh run download <RUN_ID> --pattern 'wash-*' --dir artifacts/`, then `gh release
     create vX.Y.Z artifacts/*/* --target <MERGE_SHA> --generate-notes`).
   - Dispatch the Homebrew tap update and run winget-releaser as documented in
     `release-tag.yml`.
5. The `wash-runtime` workflow fires on the `vX.Y.Z` tag push and publishes the crate
   (idempotent if already published); `charts.yml` repackages and pushes the chart.

Do not bypass step 3 — promoting a digest that hasn't had the canary suite pass defeats the
"tested what we're publishing" guarantee.

## Release artifacts at a glance

| Component | What gets published on `vX.Y.Z` |
|-----------|---------------------------------|
| `wash` (CLI) | Cross-platform binaries with SLSA provenance, GitHub Release, `ghcr.io/wasmcloud/wash:vX.Y.Z`, Homebrew tap update, winget |
| `wash-runtime` (library crate) | crates.io |
| `runtime-gateway` | `ghcr.io/wasmcloud/runtime-gateway:vX.Y.Z` |
| `runtime-operator` | `ghcr.io/wasmcloud/runtime-operator:vX.Y.Z`, Go module tag `runtime-operator/vX.Y.Z` |
| Helm chart | `ghcr.io/wasmcloud/charts/runtime-operator:vX.Y.Z` |

All five ship from a single `vX.Y.Z` tag and are co-versioned.

## Canary builds

On every merge to `main` (no tag required):

- `ghcr.io/wasmcloud/wash:canary` and `ghcr.io/wasmcloud/wash:sha-<full-sha>`
- `ghcr.io/wasmcloud/runtime-gateway:canary` and `:sha-<full-sha>`
- `ghcr.io/wasmcloud/runtime-operator:canary` and `:sha-<full-sha>`
- Helm chart `runtime-operator:v2-canary`

The `sha-<full-sha>` tags are the stable handles `release-tag.yml` pins to — the `canary`
tag moves on every subsequent main push and would race the train.

The `canary-binaries` matrix job in `wash.yml` runs only on merge commits whose subject starts
with `release: v` (i.e. release-train PR merges). It produces the cross-platform `wash`
binaries as workflow artifacts that `release-tag.yml` later attests and attaches to the
GitHub Release. Skipped on every other main push to avoid burning ~7 runners on every merge.

## Build-once-promote and tested artifacts

The release flow is designed so that **the bytes users pull are the bytes that passed CI**:

- **Container images.** Canary builds publish multi-arch manifests pinned by `sha-<full-sha>`.
  `release-tag.yml` retags those exact digests as `vX.Y.Z` via `docker buildx imagetools
  create`. No rebuild — same manifest, same content. The release tag is then re-attested.
- **wash binaries.** `canary-binaries` builds the matrix on the release commit (validated by
  `check`/`lint`/`runtime-operator-e2e`). `release-tag.yml` downloads those artifacts and
  attaches them to the GitHub Release directly, with a fresh SLSA attestation.
- **wash-runtime crate.** Published from source at the tag (`wash-runtime.yml`), with the
  Cargo.toml version verify gate — the version on the tag must match the version in source.
  Since the workspace version is bumped by the train and validated by canary CI before the
  tag is pushed, the published crate matches what was tested.
- **Helm chart.** Repackaged from source at the tag (`charts.yml`'s `release` job). Charts are
  deterministic templates; rebuild risk is low. (If we want bit-identical charts we'd add an
  ORAS-based retag step here, but it's not strictly necessary for safety.)

### Image attestations

**Attestation is release-only.** Canary container pushes intentionally skip attestation —
users only ever pull tagged releases, so attesting every main commit's canary would just be
noise (and a pointless mint of Sigstore certificates). When `release-tag.yml` retags the
canary digest as `vX.Y.Z`, it mints a fresh SLSA provenance attestation against the promoted
manifest digest, signed via Fulcio, linking that digest to the workflow run, source commit,
and runner.

Verify any released image with:
```bash
gh attestation verify --repo wasmCloud/wasmCloud oci://ghcr.io/wasmcloud/wash:vX.Y.Z
gh attestation verify --repo wasmCloud/wasmCloud oci://ghcr.io/wasmcloud/runtime-gateway:vX.Y.Z
gh attestation verify --repo wasmCloud/wasmCloud oci://ghcr.io/wasmcloud/runtime-operator:vX.Y.Z
```

Binaries on the GitHub Release have equivalent attestations (verifiable by digest):
```bash
gh attestation verify --repo wasmCloud/wasmCloud ./wash
```

### When canary fails post-merge

`canary-binaries` and the canary docker jobs only fire on push-to-main, not on the release-PR
itself (the matrix build is too expensive to run on every PR, and only release-train PRs need
it). That means a release commit can land on `main` and *then* fail the matrix build. When
this happens:

- `release-tag.yml` will fail at `Wait for canary builds on the merge commit` (stage
  `wait-for-canary`) and Slack-page `#release-bot`.
- The release commit is already on `main`, so `Cargo.toml`/`Cargo.lock`/`Chart.yaml` already
  show the bumped version.
- **Recovery: fix forward, do not revert.** Open a PR fixing the build issue with subject
  `release: vX.Y.Z` (same version — the bump is already in main). Rebase-merge it.
  The new merge commit on main will trigger a fresh canary; once it goes green, re-run
  `release-tag.yml` (workflow_dispatch, or just wait — `workflow_run` doesn't retrigger
  automatically, so manual re-dispatch is required).
- If the fix is large enough that you want a different version on the failed attempt, revert
  the release commit and run the train again with `bump=patch`. The `vX.Y.Z` git tag has not
  been pushed (release-tag never got past `wait-for-canary`), so versions are still available.

The train's "no skip" policy applies here too: don't try to dodge the failed canary by
pushing the tag manually. The promote-don't-rebuild guarantee depends on a green canary on
the exact merge commit.

### Tag immutability

- **Git tags** (`vX.Y.Z`) are protected by the [tag ruleset](#ruleset-release-tags): no force
  push, no update, no delete. `release-tag.yml` checks `git ls-remote` before pushing and is a
  no-op if the tag exists.
- **OCI tags** are not registry-immutable on GHCR, but the workflow contract makes them so:
  only `release-tag.yml` writes `vX.Y.Z`, and the promote step skips if the OCI tag already
  exists. Hand-pushes to those tags should be blocked by package-level admin permissions —
  see [Repo prerequisites](#repo-prerequisites).
- **GitHub Release** is created at the tag with a fresh attestation for the binaries; if the
  release already exists, the workflow logs an error and the maintainer chooses whether to
  republish.

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

## Release candidates

The train cuts RCs the same way it cuts GA releases — same PR shape, same canary gating, same
build-once-promote, same immutable tag. The only differences are:

- Tag is `vX.Y.Z-rc.N` instead of `vX.Y.Z`.
- The GitHub Release is marked `prerelease: true` (so `gh release list` and the GitHub UI
  don't surface it as the project's "latest" release).
- `homebrew` and `winget` updates are skipped (those distros are for stable users).
- Everything else publishes: OCI images at `vX.Y.Z-rc.N` (with SLSA attestation), the Helm
  chart at `vX.Y.Z-rc.N`, the Go module tag `runtime-operator/vX.Y.Z-rc.N`, and the
  `wash-runtime` crate at `X.Y.Z-rc.N` on crates.io.

### Cutting an RC

RCs are always cut via `workflow_dispatch` on `release-train.yml`. The biweekly cron only
cuts patches off GA — it never produces RCs.

| Want | Dispatch input |
|---|---|
| First RC of a new minor (e.g. `2.0.5` → `2.1.0-rc.1`) | `bump=rc` |
| Next RC iteration (e.g. `2.1.0-rc.1` → `2.1.0-rc.2`) | `bump=rc` |
| Finalize current RC (e.g. `2.1.0-rc.3` → `2.1.0`) | `version=2.1.0` |
| Major-version RC, or any non-standard version | `version=X.Y.Z-rc.N` |

The train auto-detects whether the current workspace version is GA or RC and picks the
right transition for `bump=rc`. Plain `bump=patch/minor/major` refuses to run when the
current version is an RC — finalize first by dispatching with the explicit `version`
override (the version is already in `Cargo.toml` on main, so the cut stays unambiguous).

### Typical RC cycle

```
2.0.5 (GA)
  └─ workflow_dispatch bump=rc        → 2.1.0-rc.1
  └─ canary CI green, soak 24h
  └─ workflow_dispatch bump=rc        → 2.1.0-rc.2  (after a fix)
  └─ canary CI green, soak 24h
  └─ workflow_dispatch version=2.1.0  → 2.1.0      (finalize; promote with no rebuild)
2.1.0 (GA)
  └─ next cron Tuesday: bump=patch    → 2.1.1
```

Each RC goes through the full canary-validate-then-promote pipeline, so an RC tag
(`v2.1.0-rc.1`) carries the same SLSA-attested bytes that the eventual GA tag will carry
if no fixes land in between. If a fix lands between the last RC and the finalize, that fix
gets its own canary cycle as part of the GA cut.

### What the cron does during an RC cycle

If a release commit on main is currently an RC (e.g. `2.1.0-rc.1`) when the Tuesday cron
fires, `bump=patch` will refuse with a Slack page: "current version is a release candidate;
finalize it by dispatching with version=X.Y.Z, or cut another RC with bump=rc." This is
intentional — the train won't bury an in-flight RC under a quiet patch. Maintainers should
respond by either:

- Dispatching `version=X.Y.Z` to finalize the in-flight RC into a GA, OR
- Dispatching `bump=rc` for the next RC iteration, OR
- Dispatching `version=X.Y.Z` for a different version if the RC was abandoned.

## Checklist after release

- [ ] All release workflows completed successfully in GitHub Actions
- [ ] GitHub Release is visible with correct binaries attached
- [ ] Docker images are available on GHCR
- [ ] Homebrew tap updated (check [homebrew-wasmcloud](https://github.com/wasmCloud/homebrew-wasmcloud))
- [ ] `wash-runtime` crate is visible on [crates.io](https://crates.io/crates/wash-runtime)
- [ ] Announce the release in [Slack](https://slack.wasmcloud.com) `#announcements`

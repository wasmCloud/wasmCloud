# Private parser fuzzing

These files are deployed to the private `wasmCloud/security-fuzz` repository:

| Source | Destination |
| --- | --- |
| `fuzz.yml` | `.github/workflows/fuzz.yml` |
| `release-fuzz.sts.yaml` | `.github/chainguard/release-fuzz.sts.yaml` |
| `policy.json` | `policy.json` |
| `write-result.mjs` | `write-result.mjs` |

The repository must remain private, with access limited to the org maintainers
and organization owners. Forking is disabled. Protect `main` with required
pull requests and non-author review. Do not enable Actions access from other
repositories: the public release workflow dispatches a separate private run.

## Authentication

Add `wasmCloud/security-fuzz` to the existing Octo STS installation's selected
repositories. The `release-fuzz` trust policy grants only `actions: write` and
`contents: read` on this private repository to the public release workflow on
`main`, through its reusable `fuzz.yml`. The repository ID and both workflow
identities are checked. No stored credentials or callback app are needed.

The gate refreshes its short-lived app token during long builds and revokes it
when finished. Private fuzz workers have read-only repository access and no
OIDC permission or release credentials.

## Harness revision

`FUZZ_HARNESS_SHA` is an optional private repository variable containing a full
trusted commit SHA from `wasmCloud/wasmCloud`. Set it to a reviewed harness
revision when bootstrapping before this PR merges. Otherwise the workflow
resolves public `main` once. Source commits must be reachable from public
`main`; harness commits may be explicitly pinned to a reviewed branch commit.
After the public integration merges, delete the bootstrap variable or advance
it whenever reviewed harnesses change.

The source checkout's lockfiles and parser implementations are used. Staging
adds only harnesses, seeds, and the router shim. Incompatible APIs or dependency
drift fail the run.

## Release protocol

The public gate sends the full candidate SHA and a unique request ID. The
private workflow resolves the source, loads the matrices from `policy.json`,
and runs every target for `release_seconds`. The result job requires both
matrices to succeed before uploading `fuzz-result-1` with `result.json`.
Pushes and pull requests validate the control files without discovery fuzzing.

The public gate pins private `main` before dispatch. It requires that exact
workflow revision, the unique request, the first run attempt, a successful
conclusion, and evidence matching the candidate and complete target policy.
There is no fallback to weekly results. A private workflow update racing with
dispatch, a rerun, or any missing result closes the gate; retry the release
workflow to request a fresh run. The gate waits up to 150 minutes and requests
cancellation on timeout. A cancelled public job may leave a bounded private
campaign running.

The public job reads only workflow metadata and the small result artifact.
Failure messages contain no private API response bodies or diagnostics.
Crash artifacts, corpora, build caches, and job logs stay in the private
repository. Weekly runs use `weekly_seconds` and do not approve releases.

## Deployment and verification

Copy the four files above into a checkout of the private repository, commit
with a DCO sign-off, and push through its review process. Policy changes must
be deployed on both sides before releasing; mismatches fail closed.

Validate from the public repository:

```sh
node --test .github/scripts/release-fuzz.test.mjs
cargo test --locked -p xtask fuzz
actionlint -shellcheck= -pyflakes= fuzz/private/fuzz.yml
zizmor --offline fuzz/private/fuzz.yml
```

Dispatch a real private campaign before enabling promotion. Confirm all 18
targets pass and that `result.json` names the selected source SHA and harness
SHA. Then verify the public gate's OIDC exchange from the merged release
workflow. The trust policy intentionally rejects PR branches and local tokens.
Before merge, maintainers can exercise the same gate protocol locally with an
authorized GitHub token injected directly into the exported `gate` function.

This implements release-specific dynamic analysis for the OpenSSF Best
Practices Gold criterion. OSPS Baseline does not prescribe fuzzing or this
release gate. Keep execution evidence and triage findings under the project's
security policy; a configured workflow alone is not evidence of a completed run.

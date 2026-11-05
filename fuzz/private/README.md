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
pull requests, review, and control-file checks. The ruleset permits `ricochet`
to bypass requirements when merging a PR. Do not enable Actions access from
other repositories: the public release train dispatches a separate private run.

## Authentication

Add `wasmCloud/security-fuzz` to the existing Octo STS installation's selected
repositories. The `release-fuzz` trust policy grants only `actions: write` and
`contents: read` on this private repository to `release-train.yml` on public
`main`. The repository ID and workflow identity are checked. No stored
credentials or callback app are needed.

The release train mints a short-lived app token with `octo-sts/action`,
which revokes it when the job finishes. Private fuzz workers have read-only repository access and no OIDC
permission or release credentials.

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
drift fail the private run.

Maintainers may manually dispatch a private branch to validate control changes.
Rust fuzzing explicitly targets GNU libc on Linux so the installed `cargo-fuzz`
binary's own build target cannot select static musl for ASan.

## Release dispatch

After its PR merges, the public release train sends the exact merge SHA and a
unique request ID. It checks the private repository's ID, visibility, and
workflow, then returns as soon as GitHub accepts the dispatch. The step has a
two-minute timeout and tolerates errors. It does not poll runs or download
results. Dispatch failures and private campaign results do not block promotion.

The private workflow resolves the source, loads the matrices from `policy.json`,
and runs every target for `release_seconds`. The result job requires both
matrices to succeed before uploading `fuzz-result-1` with `result.json`.
Evidence records the source, harness and workflow SHAs, request and run IDs,
target policy, and duration. Pushes and PRs validate control files without
discovery fuzzing. Weekly runs use `weekly_seconds`.

Private API response bodies and diagnostics stay out of public dispatch errors.
Crash artifacts, corpora, build caches, job logs, and evidence stay in the
private repository. Maintainers review results and triage findings under the
security policy. Manual release-tag recovery does not dispatch another campaign;
dispatch the private workflow with the full release SHA when needed.

## Deployment and verification

Copy the four files above into a checkout of the private repository, commit
with a DCO sign-off, and push through its review process. Deploy the release-train
trust policy before the public integration merges.

Validate from the public repository:

```sh
node --test .github/scripts/release-fuzz.test.mjs
cargo test --locked -p xtask fuzz
actionlint -shellcheck= -pyflakes= fuzz/private/fuzz.yml
zizmor --offline fuzz/private/fuzz.yml
```

Dispatch a real private campaign and confirm all 18 targets pass and that
`result.json` names the selected source and harness SHAs. Verify the OIDC
exchange from the merged release-train workflow. The trust policy intentionally
rejects PR branches and local tokens. Before merge, maintainers can test the
exported `dispatch` function locally with an authorized GitHub token.

Keep completed execution evidence for dynamic-analysis assessments. Asynchronous
dispatch alone does not establish pre-release analysis for OpenSSF Best Practices
Gold. OSPS Baseline does not prescribe fuzzing or a release gate.

# Release Runbook

This guide should serve as a runbook for maintainers who are preparing to release new patches or
features to existing crates in this repo.

This monorepo has two primary output binaries: `wasmcloud` and `wash`. Both of these binaries
share common dependencies in libraries, located under `crates/`. When releasing new versions of
`wasmcloud` and `wash`, it can be difficult to look back at the changes in the repository to
know exactly when to patch or minor bump a crate (major bumps will likely be rejected at PR unless
we're prepared to update a crate with a major bump) and in what order to release new versions of
crates. We use the tool [cargo smart-release](https://github.com/Byron/cargo-smart-release) to
automatically calculate the required semver changes and crates that must be released.

## cargo smart-release
We use [cargo smart-release](https://github.com/Byron/cargo-smart-release) as a tool to specify a
crate to release and allow our usage of [conventional
commits](https://www.conventionalcommits.org/en/v1.0.0/) and semver adherence to automatically bump,
generate CHANGELOGs, and release all necessary crates. The release process is captured in a GitHub
Actions based workflow that drives the use of `cargo smart-release`.

## Releasing a crate (library)
Crates should be released by navigating to the [smart-release
Actions](https://github.com/wasmCloud/wasmCloud/actions) pane, triggering the **wasmCloud** workflow
with the `workflow_dispatch` and selecting the crate you wish to release.

![image](https://github.com/user-attachments/assets/42af9d71-850a-4fd6-9881-d3d835d547e1)

To release a crate, select it from the dropdown list and leave the box unchecked. The action will
calculate the required necessary changes, update the CHANGELOGs, and open a pull request which can
be checked, approved, and merged. After merging that PR, run the action again but with the box
checked, which won't update CHANGELOG entries and will publish tags and crates that need to be
published. Finally, `smart-release` will create a GitHub release for the desired crate including the
full changelog.

**Detailed steps:**
1. Navigate to the <ins>[wasmcloud GitHub
   workflow](https://github.com/wasmCloud/wasmCloud/actions/workflows/wasmcloud.yml)</ins> and run
   the workflow manually, selecting the crate to release and leaving the box **unchecked**
2. Watch for the incoming PR, which should be created in 2-3 minutes
3. (Temporary) Checkout that branch, run a `git rebase HEAD~2 --signoff` to sign off the commit, and
   then `git push --force-with-lease` to sign off the smart-release commit. This ensures releases
   are driven and signed off by a maintainer.
4. Review the PR to ensure that the desired crate version is adjusted along with any expected
   dependent crates, CHANGELOGs are generated, and workflows and tests pass.
5. Approve and merge the PR
6. Navigate to the <ins>[wasmcloud GitHub
   workflow](https://github.com/wasmCloud/wasmCloud/actions/workflows/wasmcloud.yml)</ins> and run
   the workflow manually, selecting the crate to release and **check** the box to trigger the
   release
7. Watch for your crates to release and tags to create

## Releasing `wash`

1. Follow the steps above to release the `wash` crate
2. Approve the PR in the [homebrew](https://github.com/wasmCloud/homebrew-wasmcloud) repository.
   This might require signing off the commit in order to kick off the release actions. Then, attach
   the `pr-pull` label to trigger the homebrew release (this will end with the PR being merged to
   `main`.)
3. Approve and merge the PR in the [chocolatey](https://github.com/wasmCloud/chocolatey-wash)
   repository. This might require signing off the commit in order to kick off the release actions.

## Releasing `wasmcloud`
wasmCloud is released separately from the library release process and is fully automated. This
requires tag `push` access to the repository and can be performed by any @wasmCloud/org-maintainers.
1. Create a pull request updating the version in `Cargo.toml`
2. Merge the pull request to `main`
3. Create a tag with the version to release, `vX.Y.Z`, and push the tag to the repository
4. The `wasmcloud` action will trigger and will create a draft release with release artifacts
   attached.
5. Publish the release

## Releasing a capability provider
Each capability provider, which is a native executable plugin for wasmCloud, is released by pushing
a tag to the repository.

1. Create a pull request updating the capability provider version in
   `crates/provider-<provider>/Cargo.toml` and the version in
   `src/bin/provider-<provider>/wasmcloud.toml`
2. Merge the pull request to `main`
3. Create a tag with the version to release, `provider-<provider>-vX.Y.Z`, and push the tag to the
   repository
4. The `wasmcloud` action will trigger, build + package the provider, and publish as a
   [package](https://github.com/orgs/wasmCloud/packages?repo_name=wasmCloud)

## Releasing an example component
Each example component, which is a build WebAssembly component for use in example applications, is
released by pushing a tag to the repository.

1. Create a pull request updating the example version in
   `examples/<language>/components/<example>/Cargo.toml`
2. Merge the pull request to `main`
3. Create a tag with the version to release, `component-<example>-vX.Y.Z`, and push the tag to the
   repository
4. The `wasmcloud` action will trigger, build + package the provider, and publish as a
   [package](https://github.com/orgs/wasmCloud/packages?repo_name=wasmCloud)

## GitHub Releases

All core projects have GitHub releases that point at their given tag. These are created
automatically at deploy time.

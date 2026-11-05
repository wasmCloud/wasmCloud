# wasmCloud VEX assessments

[`wasmcloud.openvex.json`](wasmcloud.openvex.json) supplements upstream vulnerability reports
with non-exploitability details for the advisories ignored in [`.cargo/audit.toml`](../.cargo/audit.toml).
It uses [OpenVEX 0.2.0](https://github.com/openvex/spec/blob/main/OPENVEX-SPEC.md) to address
[OSPS-VM-04.02](https://baseline.openssf.org/versions/2026-08-28#osps-vm-0402).

Each statement identifies an advisory, the release's wash binaries (`pkg:github` package URLs)
and wash images (`pkg:oci` package URLs), `not_affected` status, justification, and supporting
source/build evidence. The Go gateway and operator images do not contain these Rust dependencies.
The `osv-scanner.toml` ignores cover examples, templates, and test fixtures, outside released code.

The [security policy](../SECURITY.md#vulnerability-exploitability-exchange-vex) defines review
requirements. Before adding an ignore, assess its evidence and add a statement covering the
current release's products. Unconfirmed findings must not be suppressed as non-exploitable. Run:

```sh
node --test .github/scripts/check-vex.test.mjs .github/scripts/release-vex.test.mjs
node .github/scripts/check-vex.mjs
```

The check uses the runner's Python 3.11+ standard library to parse Cargo TOML. It requires a
statement for every Cargo audit ignore and checks coverage of the current release products. It
warns when `Cargo.lock` no longer has a dependency version a statement names; reassess that
statement in the same pull request. It does not prove non-exploitability; maintainers must
review the evidence.

The release train runs `release-vex.mjs` to move each statement to the new version, and fails if
an assessed dependency moved. The release pull request lists the carried statements, and
approving it confirms each still holds. A new binary target needs its products added by hand.

Every GitHub Release includes `wasmcloud.openvex.json` and `wasmcloud.openvex.sigstore.json`
next to its SBOMs. Release tags and assets preserve past assessments. After downloading the
VEX file, verify that `release-tag.yml` on `main` attested it:

```sh
gh attestation verify wasmcloud.openvex.json --repo wasmCloud/wasmCloud \
  --cert-identity https://github.com/wasmCloud/wasmCloud/.github/workflows/release-tag.yml@refs/heads/main \
  --source-ref refs/heads/main \
  --predicate-type https://openvex.dev/ns/v0.2.0
```

Naming only the workflow is not enough: a copy of `release-tag.yml` dispatched from any other
branch signs with the same workflow path.

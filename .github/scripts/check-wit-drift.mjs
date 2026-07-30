#!/usr/bin/env node
// Detects silent WIT drift: a package whose declared version is already
// published, but whose freshly-built WIT differs from what's actually live
// at that tag.
//
// wit.yml's publish step treats a stable version as immutable and silently
// skips re-pushing it — correct for a genuinely unchanged republish, but it
// means a source change that landed without a matching version bump merges
// clean and then simply never reaches the registry, with nothing in CI ever
// having compared the two. This script is that comparison, run in `validate`
// (before publish), so drift fails the PR loudly instead of quietly no-op'ing
// on merge.
//
// For each matrix entry with a STABLE version (no `-` pre-release suffix —
// pre-release tags are documented as mutable and expected to change):
//   1. `docker manifest inspect` the destination ref. Not found means
//      nothing is published yet, so there is nothing to drift from — skip.
//   2. Pull the published wasm via `wash oci pull` and re-emit its WIT via
//      `wasm-tools component wit` — the same canonicalization
//      build-wit-matrix.mjs relies on, so doc-comment/whitespace-only source
//      edits (which wasm-tools strips from the compiled component) never
//      trigger a false positive.
//   3. Compare against the WIT of the wasm build-wit-matrix.mjs already built
//      from current source. Any difference fails the job, naming the package
//      so the fix (bump its version) is obvious from the failure alone.
//
// Requires `docker`, `wash`, and `wasm-tools` on PATH. Pulls are read-only
// against public ghcr.io packages, so no login step is needed here (unlike
// the publish job, which needs write access).

import { execFileSync } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const BUILD_DIR = 'wit-build';

const matrixJson = process.env.MATRIX;
if (!matrixJson) {
  console.error('MATRIX env var is not set');
  process.exit(1);
}
const { include: items } = JSON.parse(matrixJson);

// `docker manifest inspect` distinguishes "doesn't exist yet" from a real
// failure (network blip, auth issue) the same way oci-push-with-digest does —
// an ambiguous error is not license to silently treat the ref as unpublished.
function manifestExists(ref) {
  try {
    execFileSync('docker', ['manifest', 'inspect', ref], { stdio: 'pipe' });
    return true;
  } catch (err) {
    const stderr = String(err.stderr ?? '');
    if (/manifest unknown|no such manifest|not found/i.test(stderr)) {
      return false;
    }
    console.error(`destination check for ${ref} failed — refusing to compare\n${stderr}`);
    process.exit(1);
  }
}

function canonicalWit(path) {
  return execFileSync('wasm-tools', ['component', 'wit', path], {
    encoding: 'utf8',
  }).trim();
}

const tmp = mkdtempSync(join(tmpdir(), 'wit-drift-'));
let drifted = false;

for (const { name, version, ref, artifact } of items) {
  if (version.includes('-')) {
    console.log(`${ref}: pre-release, mutable by design — skipping drift check`);
    continue;
  }
  if (!manifestExists(ref)) {
    console.log(`${ref}: not yet published — nothing to drift from`);
    continue;
  }

  const pulled = join(tmp, `${name}.wasm`);
  execFileSync('wash', ['oci', 'pull', ref, pulled], { stdio: 'pipe' });

  const publishedWit = canonicalWit(pulled);
  const builtWit = canonicalWit(join(BUILD_DIR, artifact));

  if (publishedWit !== builtWit) {
    drifted = true;
    console.error(
      `${ref} is already published and stable versions are immutable, so publish will ` +
        `silently skip it — but the WIT built from source no longer matches what's live. ` +
        `Bump the package version in wit/${name}/wit/*.wit.`,
    );
  } else {
    console.log(`${ref}: matches published — no drift`);
  }
}

if (drifted) {
  process.exit(1);
}

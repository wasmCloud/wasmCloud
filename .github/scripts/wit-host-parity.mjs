#!/usr/bin/env node
// Verifies that every vendored copy of the `wasmcloud:host` WIT package (the
// host-provided component-plugin interfaces: `identity`, `cancel`) is
// byte-identical to the canonical copy under `wit/host/wit/`.
//
// These interfaces are defined in Rust in the runtime (they are installed on a
// plugin's linker by `install_host_identity` / `install_host_cancel`), so the
// WIT is only consumed by guest components for their bindings. That makes it easy
// for a vendored fixture copy to drift from the canonical shape — the exact
// divergence that let a stale interface ship unnoticed before. Fixtures that vendor
// their own `wit/deps` (e.g. `kv-plugin`, which is in xtask's `P3_SKIP_SHARED_WIT`)
// keep a local `wasmcloud-host/` copy rather than receiving one from the shared
// `p3-wit-deps/`, so this check pins them to the canonical package instead.
//
// Fails (non-zero exit) if any vendored copy is missing a canonical file, has an
// extra file, or differs in bytes.

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const CANONICAL_DIR = 'wit/host/wit';
const SKIP_DIRS = new Set(['.git', 'target', 'node_modules', 'dist']);

// Load the canonical package: filename -> bytes.
const canonical = new Map();
for (const name of readdirSync(CANONICAL_DIR)) {
  if (name.endsWith('.wit')) {
    canonical.set(name, readFileSync(join(CANONICAL_DIR, name), 'utf8'));
  }
}
if (canonical.size === 0) {
  console.error(`no .wit files found in canonical dir ${CANONICAL_DIR}`);
  process.exit(1);
}

// Find every vendored `wasmcloud-host/` directory (excluding the canonical one).
function findVendoredDirs(dir, out) {
  for (const entry of readdirSync(dir)) {
    if (SKIP_DIRS.has(entry)) continue;
    const path = join(dir, entry);
    if (!statSync(path).isDirectory()) continue;
    if (entry === 'wasmcloud-host') {
      out.push(path);
    } else {
      findVendoredDirs(path, out);
    }
  }
  return out;
}

const vendored = findVendoredDirs('.', []).filter(
  (d) => relative(CANONICAL_DIR, d) !== '',
);

const problems = [];
for (const dir of vendored) {
  const files = readdirSync(dir).filter((n) => n.endsWith('.wit'));
  for (const name of files) {
    if (!canonical.has(name)) {
      problems.push(`${join(dir, name)}: not part of the canonical package (${CANONICAL_DIR})`);
      continue;
    }
    const bytes = readFileSync(join(dir, name), 'utf8');
    if (bytes !== canonical.get(name)) {
      problems.push(`${join(dir, name)}: differs from canonical ${join(CANONICAL_DIR, name)}`);
    }
  }
  for (const name of canonical.keys()) {
    if (!files.includes(name)) {
      problems.push(`${join(dir, name)}: missing (present in canonical ${CANONICAL_DIR})`);
    }
  }
}

if (problems.length > 0) {
  console.error('wasmcloud:host WIT copies are out of sync with the canonical package:\n');
  for (const p of problems) console.error(`  - ${p}`);
  console.error(`\nUpdate the copy to match ${CANONICAL_DIR}/ (or vice versa) so they are identical.`);
  process.exit(1);
}

console.log(
  `wasmcloud:host WIT parity OK: ${vendored.length} vendored ` +
    `cop${vendored.length === 1 ? 'y' : 'ies'} match ${CANONICAL_DIR}/`,
);

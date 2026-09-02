#!/usr/bin/env node
// Holds the host's wasmtime feature set to what wash-runtime declares, on
// every target we ship a binary for.
//
// Cargo features are additive and unify across the whole graph, so a
// dependency that takes `wasmtime` with default features turns those defaults
// on for everyone. `wasi-webgpu-wasmtime` does exactly that — and it is
// target-gated off Windows and s390x (wgpu-hal does not build there), so any
// feature only it supplies is present in six release binaries and missing from
// two. That is invisible in a normal build: the host still compiles and runs,
// it just compiles Wasm on one thread, or picks the null garbage collector
// that never reclaims, on the two targets nobody builds locally.
//
// Two invariants, both cheap — `cargo tree` resolves without compiling:
//
//   1. Everything in REQUIRED is declared by wash-runtime itself, not
//      inherited. This is what stops a dependency from owning our feature set:
//      if webgpu is the only reason a feature is on, this fails.
//   2. Everything in REQUIRED actually resolves on every release target. This
//      catches target-gating and any other resolution surprise.
//
// Features a target picks up beyond REQUIRED are reported, not failed —
// webgpu legitimately adds a dozen on the targets it builds for.
//
// Adding a feature to the manifest means adding it here with its reason.

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const MANIFEST = 'crates/wash-runtime/Cargo.toml';
// The release matrix is the authority on what we ship; reading it back means a
// new target is covered here the moment it is added there.
const WORKFLOW = '.github/workflows/wash.yml';
// The package whose resolved graph decides what a released binary contains.
const PACKAGE = 'wash';

// Why each one is load-bearing. A reader who wants to drop one should have to
// argue with the reason first.
const REQUIRED = new Map([
  ['addr2line', 'turns a trapping address back into guest source file and line'],
  ['anyhow', 'wasmtime errors interoperate with the host error type'],
  ['backtrace', 'host Rust backtrace on a wasmtime::Error'],
  ['component-model', 'the host runs components, not core modules'],
  ['component-model-async', "WASIP3's async ABI"],
  ['cranelift', 'components are compiled on the host, not loaded precompiled'],
  ['demangle', 'readable symbol names in trap frames'],
  ['gc', 'wasm_gc is on by default in wasmtime, so guests can use GC types'],
  ['gc-copying', 'the collector Collector::Auto resolves to first'],
  ['gc-drc', 'the collector Auto falls back to; without both, Auto reaches gc-null, which collects nothing'],
  ['gc-null', 'keeps the no-op collector explicitly selectable'],
  ['parallel-compilation', 'Cranelift compiles a component across rayon’s pool'],
  ['pooling-allocator', 'the engine installs a PoolingAllocationConfig'],
  ['threads', 'shared memories'],
]);

// The `features = [...]` array on wash-runtime's own `wasmtime` dependency.
function declaredFeatures() {
  const manifest = readFileSync(MANIFEST, 'utf8');
  const dep = manifest.match(/^wasmtime = \{[^}]*\}/m);
  if (!dep) {
    console.error(`could not find the \`wasmtime\` dependency line in ${MANIFEST}`);
    process.exit(1);
  }
  const features = dep[0].match(/features = \[([^\]]*)\]/);
  if (!features) {
    console.error(`the \`wasmtime\` dependency in ${MANIFEST} declares no feature list`);
    process.exit(1);
  }
  return new Set([...features[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]));
}

function releaseTargets() {
  const workflow = readFileSync(WORKFLOW, 'utf8');
  const targets = [...workflow.matchAll(/^\s+- target: (\S+)$/gm)].map((m) => m[1]);
  if (targets.length === 0) {
    console.error(`found no \`- target:\` entries in ${WORKFLOW}`);
    process.exit(1);
  }
  return [...new Set(targets)];
}

// `-i wasmtime` inverts the tree onto wasmtime, so every enabled feature of it
// appears as its own node. `--prefix none` drops the tree drawing, leaving one
// `wasmtime feature "x"` per line (repeated once per enabling edge).
function resolvedFeatures(target) {
  const out = execFileSync(
    'cargo',
    ['tree', '--target', target, '-p', PACKAGE, '-e', 'features', '-i', 'wasmtime', '--prefix', 'none'],
    { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 },
  );
  return new Set([...out.matchAll(/^wasmtime feature "([^"]+)"$/gm)].map((m) => m[1]));
}

let failed = false;

const declared = declaredFeatures();
const undeclared = [...REQUIRED.keys()].filter((f) => !declared.has(f));
if (undeclared.length > 0) {
  failed = true;
  console.error(
    `::error::${MANIFEST} does not declare ${undeclared.join(', ')} on its \`wasmtime\` ` +
      `dependency. A transitive dependency may be supplying it today, which means the ` +
      `targets that dependency is gated off silently lose it. Declare it here.`,
  );
}
for (const feature of declared) {
  if (!REQUIRED.has(feature)) {
    console.log(`${MANIFEST} declares "${feature}", which this check does not know about`);
  }
}

for (const target of releaseTargets()) {
  const resolved = resolvedFeatures(target);
  const missing = [...REQUIRED.keys()].filter((f) => !resolved.has(f));
  if (missing.length > 0) {
    failed = true;
    for (const feature of missing) {
      console.error(
        `::error::${target} resolves without wasmtime's "${feature}" feature — ${REQUIRED.get(feature)}`,
      );
    }
  } else {
    const extra = [...resolved].filter((f) => !REQUIRED.has(f)).sort();
    console.log(`${target}: all ${REQUIRED.size} required present${extra.length > 0 ? `, plus ${extra.join(', ')}` : ''}`);
  }
}

if (failed) {
  process.exit(1);
}

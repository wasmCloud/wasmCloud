// The release products a VEX statement must cover, and the Cargo inputs that
// decide them, shared by check-vex.mjs and release-vex.mjs.

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

export const VEX_PATH = 'vex/wasmcloud.openvex.json';

// The binary artifact names a release attaches, from wash.yml's build matrix.
export function releaseArtifacts(washWorkflow) {
  const artifacts = [...new Set([...washWorkflow.matchAll(/^\s+artifact: (wash-[\w-]+)\s*$/gm)].map((m) => m[1]))];
  if (!artifacts.length) throw new Error('no release binary targets found in wash.yml');
  return [...artifacts, 'wash-x86_64-pc-windows-msvc.zip'];
}

// Package URLs for every binary and wash image a release ships.
export function releaseProducts(version, artifacts) {
  return [
    ...artifacts.map((name) => `pkg:github/wasmcloud/wasmcloud@v${version}?file_name=${name}`),
    ...['', '-all-features'].map((suffix) =>
      `pkg:oci/wash@${version}${suffix}?repository_url=ghcr.io%2Fwasmcloud%2Fwash`),
  ];
}

// Splits `pkg:cargo/<name>@<version>` into its parts, or returns null.
export function cargoPurl(id) {
  const match = /^pkg:cargo\/([^@/?#]+)@([^?#]+)$/.exec(id);
  return match ? { name: match[1], version: match[2] } : null;
}

// Each cargo subcomponent of a `version` product that Cargo.lock does not
// contain at that exact version: the dependency moved since the assessment.
// Only statements for advisories in `ignored` count, since only those still
// suppress a finding.
export function lockDrift(doc, version, artifacts, lock, ignored) {
  const required = new Set(releaseProducts(version, artifacts));
  const suppressing = new Set(ignored);
  const drift = new Map();
  for (const statement of doc.statements ?? []) {
    if (!suppressing.has(statement.vulnerability?.name)) continue;
    for (const product of statement.products ?? []) {
      if (!required.has(product['@id'])) continue;
      for (const sub of product.subcomponents ?? []) {
        const pkg = cargoPurl(sub['@id']);
        if (!pkg || lock.get(pkg.name)?.has(pkg.version)) continue;
        const key = `${statement.vulnerability?.name} ${sub['@id']}`;
        drift.set(key, {
          advisory: statement.vulnerability?.name,
          subcomponent: sub['@id'],
          locked: [...(lock.get(pkg.name) ?? [])].sort(),
        });
      }
    }
  }
  return [...drift.values()];
}

export function describeDrift({ advisory, subcomponent, locked }) {
  const now = locked.length ? `Cargo.lock has ${locked.join(', ')}` : 'Cargo.lock no longer has it';
  return `${advisory} was assessed against ${subcomponent}, but ${now}`;
}

// The audit and OSV-Scanner ignores, workspace version and locked packages under `root`,
// parsed with the runner's Python TOML reader for Cargo's quoting and arrays.
export function readCargo(root) {
  const { ignored, osvIgnored, version, packages } = JSON.parse(execFileSync('python3', ['-c', `
import json, pathlib, tomllib
audit = tomllib.loads(pathlib.Path('.cargo/audit.toml').read_text())
manifest = tomllib.loads(pathlib.Path('Cargo.toml').read_text())
osv = pathlib.Path('osv-scanner.toml')
osv_ignored = [v['id'] for v in tomllib.loads(osv.read_text()).get('IgnoredVulns', [])] if osv.exists() else []
lock = pathlib.Path('Cargo.lock')
packages = tomllib.loads(lock.read_text()).get('package', []) if lock.exists() else []
print(json.dumps({'ignored': audit.get('advisories', {}).get('ignore', []),
                  'osvIgnored': osv_ignored,
                  'version': manifest['workspace']['package']['version'],
                  'packages': [[p['name'], p['version']] for p in packages]}))
  `], { cwd: root, encoding: 'utf8' }));
  const lock = new Map();
  for (const [name, v] of packages) {
    if (!lock.has(name)) lock.set(name, new Set());
    lock.get(name).add(v);
  }
  return { ignored, osvIgnored, version, lock };
}

export function readArtifacts(root) {
  return releaseArtifacts(readFileSync(join(root, '.github/workflows/wash.yml'), 'utf8'));
}

export function readVex(root) {
  return JSON.parse(readFileSync(join(root, VEX_PATH), 'utf8'));
}

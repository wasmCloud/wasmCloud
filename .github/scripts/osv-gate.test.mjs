import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { classify, informational } from './osv-gate.mjs';

const ROOT = '/repo';

function vuln(id, kind) {
  return {
    id,
    summary: `${id} summary`,
    affected: [{ database_specific: kind ? { informational: kind, cvss: null } : { cvss: null } }],
  };
}

function report(...packages) {
  return {
    results: [{
      source: { path: `${ROOT}/examples/http/Cargo.lock`, type: 'lockfile' },
      packages: packages.map(([name, vulnerabilities]) => ({
        package: { name, version: '1.0.0', ecosystem: 'crates.io' },
        vulnerabilities,
      })),
    }],
  };
}

test('RustSec informational advisories are warnings', () => {
  for (const kind of ['unmaintained', 'unsound', 'notice']) {
    const { blocking, warnings } = classify(report(['instant', [vuln('RUSTSEC-2024-0384', kind)]]), ROOT);
    assert.deepEqual(blocking, []);
    assert.equal(warnings.length, 1);
    assert.equal(warnings[0].kind, kind);
    assert.equal(warnings[0].source, 'examples/http/Cargo.lock');
  }
});

test('vulnerabilities proper block', () => {
  const { blocking, warnings } = classify(report(['zstd', [vuln('RUSTSEC-2026-0391')]], ['brotli', [vuln('GO-2026-6888')]]), ROOT);
  assert.deepEqual(blocking.map((f) => f.id), ['RUSTSEC-2026-0391', 'GO-2026-6888']);
  assert.deepEqual(warnings, []);
});

test('a malicious package blocks even when marked informational', () => {
  assert.equal(informational(vuln('MAL-2026-1', 'unmaintained')), null);
  const { blocking } = classify(report(['evil', [vuln('MAL-2026-1', 'unmaintained')]]), ROOT);
  assert.deepEqual(blocking.map((f) => f.id), ['MAL-2026-1']);
});

test('reports each finding once per lockfile and package version', () => {
  const doubled = report(['zstd', [vuln('RUSTSEC-2026-0391')]]);
  doubled.results.push(structuredClone(doubled.results[0]));
  assert.equal(classify(doubled, ROOT).blocking.length, 1);
});

test('an empty report passes', () => {
  assert.deepEqual(classify({ results: [] }), { blocking: [], warnings: [] });
});

test('CLI fails only on blocking findings', () => {
  const dir = mkdtempSync(join(tmpdir(), 'osv-gate-test-'));
  const script = fileURLToPath(new URL('./osv-gate.mjs', import.meta.url));
  const run = (contents) => {
    const path = join(dir, 'report.json');
    writeFileSync(path, JSON.stringify(contents));
    return spawnSync(process.execPath, [script, path], { encoding: 'utf8' });
  };
  try {
    let result = run(report(['instant', [vuln('RUSTSEC-2024-0384', 'unmaintained')]]));
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /::warning title=RustSec unmaintained advisory::RUSTSEC-2024-0384 in instant 1\.0\.0/);
    result = run(report(['zstd', [vuln('RUSTSEC-2026-0391')]]));
    assert.equal(result.status, 1);
    assert.match(result.stdout, /::error title=OSV finding::RUSTSEC-2026-0391 in zstd 1\.0\.0/);
    assert.match(result.stderr, /found 1 blocking finding\. /);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

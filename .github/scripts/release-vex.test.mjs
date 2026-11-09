import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { checkVex } from './check-vex.mjs';
import { releaseProducts } from './lib/vex.mjs';
import { carriedSummary, carryVex } from './release-vex.mjs';

const ADVISORY = 'RUSTSEC-2026-0001';
const BINARY = 'wash-x86_64-unknown-linux-gnu';
const ARTIFACTS = [BINARY, 'wash-x86_64-pc-windows-msvc.zip'];
const DEPENDENCY = 'pkg:cargo/zstd@0.13.3';
const NOW = '2026-11-05T00:00:00Z';

function document(version = '2.0.0') {
  return {
    '@context': 'https://openvex.dev/ns/v0.2.0',
    '@id': 'https://example.org/vex.json',
    author: 'https://example.org/security',
    timestamp: '2026-11-04T20:20:55Z',
    version: 1,
    statements: [{
      vulnerability: { name: ADVISORY, '@id': 'https://example.org/advisory' },
      products: releaseProducts(version, ARTIFACTS).map((id) => ({ '@id': id, subcomponents: [{ '@id': DEPENDENCY }] })),
      status: 'not_affected',
      justification: 'vulnerable_code_not_in_execute_path',
      impact_statement: 'The build disables the feature containing the affected API.',
      status_notes: 'Assessed source and feature graph.',
    }],
  };
}

function lock(entries = { zstd: ['0.13.3'] }) {
  return new Map(Object.entries(entries).map(([name, versions]) => [name, new Set(versions)]));
}

test('moves every statement to the next release', () => {
  const { doc, carried } = carryVex(document(), '2.0.0', '2.0.1', ARTIFACTS, lock(), NOW, [ADVISORY]);
  checkVex(doc, [ADVISORY], '2.0.1', ARTIFACTS);
  assert.throws(() => checkVex(doc, [ADVISORY], '2.0.0', ARTIFACTS), /lacks release product/);
  assert.deepEqual(doc.statements[0].products, document('2.0.1').statements[0].products);
  assert.deepEqual(carried.map((c) => c.name), [ADVISORY]);
  assert.equal(doc.version, 2);
  assert.equal(doc.last_updated, NOW, [ADVISORY]);
});

test('a release candidate carries to its final release', () => {
  const { doc } = carryVex(document('2.1.0-rc.1'), '2.1.0-rc.1', '2.1.0', ARTIFACTS, lock(), NOW, [ADVISORY]);
  checkVex(doc, [ADVISORY], '2.1.0', ARTIFACTS);
});

test('a moved dependency refuses to carry its assessment', () => {
  for (const entries of [{ zstd: ['0.13.4'] }, {}]) {
    assert.throws(
      () => carryVex(document(), '2.0.0', '2.0.1', ARTIFACTS, lock(entries), NOW, [ADVISORY]),
      /reassess before releasing 2\.0\.1:\nRUSTSEC-2026-0001 was assessed against pkg:cargo\/zstd@0\.13\.3/,
    );
  }
});

test('a dependency locked at several versions still carries', () => {
  carryVex(document(), '2.0.0', '2.0.1', ARTIFACTS, lock({ zstd: ['0.12.0', '0.13.3'] }), NOW, [ADVISORY]);
});

test('a statement without the prior release needs a hand assessment', () => {
  const doc = document();
  doc.statements[0].products.pop();
  assert.throws(() => carryVex(doc, '2.0.0', '2.0.1', ARTIFACTS, lock(), NOW, [ADVISORY]), /lacks release product .*assess it by hand/);
});

test('a statement already assessed for the next release is kept as written', () => {
  const doc = document();
  doc.statements[0].products.push(...document('2.0.1').statements[0].products);
  doc.statements[0].status_notes = 'Reassessed for 2.0.1.';
  const { doc: next, carried } = carryVex(doc, '2.0.0', '2.0.1', ARTIFACTS, lock(), NOW, [ADVISORY]);
  assert.deepEqual(next, doc);
  assert.deepEqual(carried, []);
});

test('summarizes carried statements for the release PR', () => {
  assert.equal(carriedSummary([], '2.0.0', '2.0.1'), '');
  const summary = carriedSummary(carryVex(document(), '2.0.0', '2.0.1', ARTIFACTS, lock(), NOW, [ADVISORY]).carried, '2.0.0', '2.0.1');
  assert.match(summary, /carried from v2\.0\.0 to v2\.0\.1/);
  assert.match(summary, /- `RUSTSEC-2026-0001`: not_affected \(`vulnerable_code_not_in_execute_path`\)/);
});

test('an advisory no longer ignored after a dependency update becomes fixed', () => {
  const { doc, carried } = carryVex(document(), '2.0.0', '2.0.1', ARTIFACTS, lock({ zstd: ['0.13.4'] }), NOW, []);
  checkVex(doc, [], '2.0.1', ARTIFACTS);
  const [statement] = doc.statements;
  assert.equal(statement.status, 'fixed');
  assert.equal(statement.justification, undefined);
  assert.equal(statement.impact_statement, undefined);
  assert.match(statement.status_notes, /Fixed in v2\.0\.1 .*zstd@0\.13\.4/);
  assert.deepEqual(statement.products, releaseProducts('2.0.1', ARTIFACTS)
    .map((id) => ({ '@id': id, subcomponents: [{ '@id': 'pkg:cargo/zstd@0.13.4' }] })));
  assert.deepEqual(carried, [{ name: ADVISORY, status: 'fixed' }]);
  assert.match(carriedSummary(carried, '2.0.0', '2.0.1'), /- `RUSTSEC-2026-0001`: fixed$/m);
});

test('a dependency the lock dropped is fixed with no subcomponents', () => {
  const { doc } = carryVex(document(), '2.0.0', '2.0.1', ARTIFACTS, lock({}), NOW, []);
  assert.equal(doc.statements[0].status, 'fixed');
  assert.deepEqual(doc.statements[0].products, releaseProducts('2.0.1', ARTIFACTS).map((id) => ({ '@id': id })));
});

test('removing an ignore without the update refuses to carry', () => {
  assert.throws(
    () => carryVex(document(), '2.0.0', '2.0.1', ARTIFACTS, lock(), NOW, []),
    /RUSTSEC-2026-0001 is no longer ignored .* still has pkg:cargo\/zstd@0\.13\.3/,
  );
});

test('a fixed statement carries with the versions the release ships', () => {
  const fixed = carryVex(document(), '2.0.0', '2.0.1', ARTIFACTS, lock({ zstd: ['0.13.4'] }), NOW, []).doc;
  const { doc } = carryVex(fixed, '2.0.1', '2.0.2', ARTIFACTS, lock({ zstd: ['0.13.5'] }), NOW, []);
  checkVex(doc, [], '2.0.2', ARTIFACTS);
  assert.equal(doc.statements[0].status, 'fixed');
  assert.deepEqual(doc.statements[0].products, releaseProducts('2.0.2', ARTIFACTS)
    .map((id) => ({ '@id': id, subcomponents: [{ '@id': 'pkg:cargo/zstd@0.13.5' }] })));
  assert.match(doc.statements[0].status_notes, /Fixed in v2\.0\.1 /);
  assert.match(doc.statements[0].status_notes, /the v2\.0\.1 Cargo\.lock has zstd@0\.13\.4\./);
  assert.doesNotMatch(doc.statements[0].status_notes, /Cargo\.lock now has/);
});

test('a later release dropping a fixed dependency keeps explicitly historical notes', () => {
  const fixed = carryVex(document(), '2.0.0', '2.0.1', ARTIFACTS, lock({ zstd: ['0.13.4'] }), NOW, []).doc;
  const { doc } = carryVex(fixed, '2.0.1', '2.0.2', ARTIFACTS, lock({}), NOW, []);
  checkVex(doc, [], '2.0.2', ARTIFACTS);
  assert.deepEqual(doc.statements[0].products, releaseProducts('2.0.2', ARTIFACTS).map((id) => ({ '@id': id })));
  assert.match(doc.statements[0].status_notes, /the v2\.0\.1 Cargo\.lock has zstd@0\.13\.4\./);
});

function repo(version, lockVersion, ignored = [ADVISORY]) {
  const root = mkdtempSync(join(tmpdir(), 'release-vex-test-'));
  for (const dir of ['.cargo', '.github/workflows', 'vex']) mkdirSync(join(root, dir), { recursive: true });
  writeFileSync(join(root, 'Cargo.toml'), `[workspace.package]\nversion = "${version}"\n`);
  writeFileSync(join(root, 'Cargo.lock'), `version = 4\n\n[[package]]\nname = "zstd"\nversion = "${lockVersion}"\n`);
  writeFileSync(join(root, '.cargo/audit.toml'), `[advisories]\nignore = ${JSON.stringify(ignored)}\n`);
  writeFileSync(join(root, 'osv-scanner.toml'), ignored.map((id) => `[[IgnoredVulns]]\nid = "${id}"\nreason = "VEX not_affected"\n`).join('\n'));
  writeFileSync(join(root, '.github/workflows/wash.yml'), `  artifact: ${BINARY}\n`);
  writeFileSync(join(root, 'vex/wasmcloud.openvex.json'), `${JSON.stringify(document(), null, 2)}\n`);
  return root;
}

function run(root, script, ...args) {
  return spawnSync(process.execPath, [fileURLToPath(new URL(script, import.meta.url)), ...args], { cwd: root, encoding: 'utf8' });
}

test('CLI moves the VEX file after the version bump and writes the summary', () => {
  const root = repo('2.0.1', '0.13.3');
  try {
    const summary = join(root, 'summary.md');
    let result = run(root, './release-vex.mjs', '--from', '2.0.0', '--to', '2.0.1', '--summary', summary);
    assert.equal(result.status, 0, result.stderr);
    assert.match(readFileSync(summary, 'utf8'), /RUSTSEC-2026-0001/);
    result = run(root, './check-vex.mjs', '--release');
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stdout, /::warning/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('CLI refuses to run before the version bump', () => {
  const root = repo('2.0.0', '0.13.3');
  try {
    const result = run(root, './release-vex.mjs', '--from', '2.0.0', '--to', '2.0.1');
    assert.equal(result.status, 1);
    assert.match(result.stderr, /bump it to 2\.0\.1 first/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('check-vex warns about a moved dependency between releases and fails a release', () => {
  const root = repo('2.0.0', '0.13.4');
  try {
    let result = run(root, './check-vex.mjs');
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /::warning title=VEX assessment out of date::RUSTSEC-2026-0001 was assessed against pkg:cargo\/zstd@0\.13\.3, but Cargo\.lock has 0\.13\.4/);
    result = run(root, './check-vex.mjs', '--release');
    assert.equal(result.status, 1);
    assert.match(result.stderr, /reassess before releasing 2\.0\.0/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('CLI retires an advisory after the dependency update removes its ignore', () => {
  const root = repo('2.0.0', '0.13.4', []);
  try {
    let result = run(root, './check-vex.mjs');
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stdout, /::warning/);
    writeFileSync(join(root, 'Cargo.toml'), '[workspace.package]\nversion = "2.0.1"\n');
    result = run(root, './release-vex.mjs', '--from', '2.0.0', '--to', '2.0.1');
    assert.equal(result.status, 0, result.stderr);
    result = run(root, './check-vex.mjs', '--release');
    assert.equal(result.status, 0, result.stderr);
    const doc = JSON.parse(readFileSync(join(root, 'vex/wasmcloud.openvex.json'), 'utf8'));
    assert.equal(doc.statements[0].status, 'fixed');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

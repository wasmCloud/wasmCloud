import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { checkVex } from './check-vex.mjs';

const ADVISORY = 'RUSTSEC-2026-0001';
const VERSION = '2.0.0';
const BINARY = 'wash-x86_64-unknown-linux-gnu';
const ARTIFACTS = [BINARY, 'wash-x86_64-pc-windows-msvc.zip'];

function document() {
  return {
    '@context': 'https://openvex.dev/ns/v0.2.0',
    '@id': 'https://example.org/vex.json',
    author: 'https://example.org/security',
    timestamp: '2026-11-04T20:20:55Z',
    version: 1,
    statements: [{
      vulnerability: { name: ADVISORY, '@id': 'https://example.org/advisory' },
      products: [
        ...ARTIFACTS.map((name) => ({ '@id': `pkg:github/wasmcloud/wasmcloud@v${VERSION}?file_name=${name}` })),
        ...['', '-all-features'].map((suffix) => ({ '@id': `pkg:oci/wash@${VERSION}${suffix}?repository_url=ghcr.io%2Fwasmcloud%2Fwash` })),
      ],
      status: 'not_affected',
      justification: 'vulnerable_code_not_in_execute_path',
      impact_statement: 'The build disables the feature containing the affected API.',
      status_notes: 'Assessed source and feature graph.',
    }],
  };
}

function check(doc, ignored = [ADVISORY], version = VERSION) {
  checkVex(doc, ignored, version, ARTIFACTS);
}

test('accepts complete non-exploitability assessments', () => check(document()));

test('rejects an ignored advisory without an assessment', () => {
  assert.throws(() => check(document(), ['RUSTSEC-2026-0002']), /without a VEX statement/);
});

test('an assessment for an old release cannot cover a new release', () => {
  assert.throws(() => check(document(), [ADVISORY], '2.1.0'), /lacks release product/);
});

test('requires every binary and both wash images', () => {
  for (let i = 0; i < document().statements[0].products.length; i++) {
    const doc = document();
    doc.statements[0].products.splice(i, 1);
    assert.throws(() => check(doc), /lacks release product/);
  }
});

test('requires non-exploitability evidence and a recognized justification', () => {
  for (const field of ['justification', 'impact_statement', 'status_notes']) {
    const doc = document();
    delete doc.statements[0][field];
    assert.throws(() => check(doc));
  }
});

test('other statuses do not authorize suppression', () => {
  for (const status of ['affected', 'fixed', 'under_investigation']) {
    const doc = document();
    doc.statements[0].status = status;
    doc.statements[0].action_statement = 'Update the dependency.';
    assert.throws(() => check(doc), /without a VEX statement/);
    check(doc, []);
  }
});

test('rejects conflicting duplicate statements', () => {
  const doc = document();
  doc.statements.push(structuredClone(doc.statements[0]));
  assert.throws(() => check(doc), /duplicate statement/);
});

test('CLI parses multiline TOML ignores with comments and literal strings', () => {
  const root = mkdtempSync(join(tmpdir(), 'check-vex-test-'));
  try {
    for (const dir of ['.cargo', '.github/workflows', 'vex']) mkdirSync(join(root, dir), { recursive: true });
    writeFileSync(join(root, 'Cargo.toml'), `[workspace.package]\nversion = "${VERSION}"\n`);
    writeFileSync(join(root, '.cargo/audit.toml'), `[advisories]\nignore = [\n# "RUSTSEC-2026-9999" is a comment\n'${ADVISORY}',\n]\n`);
    writeFileSync(join(root, '.github/workflows/wash.yml'), `artifact: ignored\n  artifact: ${BINARY}\n`);
    writeFileSync(join(root, 'vex/wasmcloud.openvex.json'), JSON.stringify(document()));
    const script = fileURLToPath(new URL('./check-vex.mjs', import.meta.url));
    const result = spawnSync(process.execPath, [script], { cwd: root, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /covers all 1 Cargo audit ignores/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

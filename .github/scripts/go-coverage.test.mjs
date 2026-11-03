import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { branchCounts, meetsThreshold, statementCounts } from './go-coverage.mjs';

test('statements are weighted by block size, including unexecuted packages', () => {
  assert.deepEqual(statementCounts([
    'mode: atomic',
    'example.org/mod/a.go:1.1,2.2 9 1',
    'example.org/mod/a.go:3.1,3.2 1 100',
    'example.org/mod/untested/b.go:1.1,4.2 10 0',
    '',
  ].join('\n')), { covered: 10, total: 20 });
});

test('invalid and empty profiles cannot supply coverage evidence', () => {
  for (const profile of ['', 'mode: atomic\n', 'mode: set\na.go:1.1,2.2 2 1', 'mode: atomic\ninvalid']) {
    assert.throws(() => statementCounts(profile));
  }
});

test('multi-package profiles merge duplicate blocks without repeating the denominator', () => {
  assert.deepEqual(statementCounts([
    'mode: atomic',
    'example.org/mod/a file.go:1.1,2.2 9 0',
    'example.org/mod/b.go:1.1,2.2 1 1',
    'example.org/mod/a file.go:1.1,2.2 9 3',
    'example.org/mod/b.go:1.1,2.2 1 0',
    'example.org/mod/untested/c.go:1.1,2.2 10 0',
    '',
  ].join('\n')), { covered: 10, total: 20 });
  assert.throws(() => statementCounts('mode: atomic\na.go:1.1,2.2 2 0\na.go:1.1,2.2 3 1'));
});

test('branch coverage counts true and false outcomes separately', () => {
  const condition = { Start: 'a.go:1:1', Code: 'a > 0' };
  assert.deepEqual(branchCounts([
    { ...condition, TrueCount: 50, FalseCount: 1 },
    { ...condition, TrueCount: 0, FalseCount: 30 },
    { ...condition, TrueCount: 0, FalseCount: 0 },
  ]), { covered: 3, total: 6 });
  assert.deepEqual(branchCounts([]), { covered: 0, total: 0 });
});

test('branch data must contain valid counters', () => {
  for (const data of [null, {}, [{}], [{ Start: 'a', Code: 'b', TrueCount: -1, FalseCount: 0 }]]) {
    assert.throws(() => branchCounts(data));
  }
});

test('threshold checks use exact counts instead of rounded percentages', () => {
  assert.equal(meetsThreshold({ covered: 89999, total: 100000 }, 90), false);
  assert.equal(meetsThreshold({ covered: 90, total: 100 }, 90), true);
  assert.equal(meetsThreshold({ covered: 80, total: 100 }, 80), true);
  assert.equal(meetsThreshold({ covered: 0, total: 0 }, 80), false);
});

test('collector retains diagnostics and rejects failed or incomplete runs', () => {
  const root = mkdtempSync(join(tmpdir(), 'go coverage test '));
  try {
    const scripts = join(root, '.github/scripts');
    const bin = join(root, 'bin');
    const module = join(root, 'runtime-gateway');
    const out = join(root, 'target/go-coverage/runtime-gateway');
    for (const dir of [scripts, bin, module]) mkdirSync(dir, { recursive: true });
    const script = join(scripts, 'go-coverage.mjs');
    copyFileSync(new URL('./go-coverage.mjs', import.meta.url), script);
    const fake = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const tool = path.basename(process.argv[1]);
const mode = process.env.FIXTURE_MODE;
if (tool === 'git') {
  if (args[0] === 'rev-parse') console.log('fixture-commit');
} else if (tool === 'go') {
  if (args[0] === 'version') console.log('go fixture');
  else if (args[0] === 'list') {
    for (const pkg of ['', '/untested', '/test/integration', '/test/e2e']) {
      console.log('example.org/mod' + pkg + '\\t' + process.cwd() + pkg + '\\t' + !pkg.startsWith('/test/'));
    }
  } else if (args[0] === 'test') {
    if (process.env.GOWORK !== 'off') throw new Error('workspace enabled');
    if (!args.includes('-covermode=atomic') || !args.includes('-count=1')) throw new Error('wrong coverage flags');
    if (!args.includes('-coverpkg=example.org/mod,example.org/mod/untested,example.org/mod/test/integration')) throw new Error('wrong denominator');
    fs.writeFileSync(args.find(a => a.startsWith('-coverprofile=')).split('=')[1],
      'mode: atomic\\nexample.org/mod/a.go:1.1,2.2 ' + (mode === 'below-statements' ? 8 : 9) + ' 1\\nexample.org/mod/a.go:1.1,2.2 ' + (mode === 'below-statements' ? 8 : 9) + ' 0\\nexample.org/mod/untested/b.go:1.1,2.2 1 0\\n');
    process.exit(mode === 'test-failure' ? 1 : 0);
  } else if (args.includes('-func')) console.log('total: 90.0%');
  else if (args.includes('-html')) fs.writeFileSync(args[args.indexOf('-o') + 1], '<html></html>');
  else throw new Error('unexpected go command');
} else if (tool === 'gobco') {
  if (args[0] === '-version') console.log('gobco fixture');
  else {
    if (args.at(-1).endsWith('/test/integration')) throw new Error('instrumented test-only package');
    if (!args.includes('-branch')) throw new Error('condition coverage requested');
    if (mode !== 'missing-branches') {
      const count = args.at(-1).endsWith('/untested') ? 1 : 5;
      fs.writeFileSync(args[args.indexOf('-stats') + 1], JSON.stringify(Array.from({ length: count }, (_, i) =>
        ({ Start: 'a.go:' + i + ':1', Code: 'x', TrueCount: 1, FalseCount: i === 0 || mode === 'below-branches' ? 0 : 1 }))));
    }
    process.exit(mode === 'branch-failure' ? 1 : 0);
  }
} else throw new Error('unexpected tool');
`;
    for (const tool of ['go', 'gobco', 'git']) {
      writeFileSync(join(bin, tool), fake);
      chmodSync(join(bin, tool), 0o755);
    }
    for (const mode of ['passing', 'below-statements', 'below-branches', 'test-failure', 'branch-failure', 'missing-branches']) {
      const result = spawnSync(process.execPath, [script, 'runtime-gateway'], {
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, GOBCO: join(bin, 'gobco'), FIXTURE_MODE: mode, GITHUB_STEP_SUMMARY: join(root, 'summary.md') },
        encoding: 'utf8',
      });
      assert.equal(result.status, mode === 'passing' ? 0 : 1, result.stderr);
      const report = JSON.parse(readFileSync(join(out, 'coverage.json'), 'utf8'));
      assert.equal(report.passed, mode === 'passing');
      assert.equal(report.complete, ['passing', 'below-statements', 'below-branches'].includes(mode));
      assert.deepEqual(report.packages, ['example.org/mod', 'example.org/mod/untested', 'example.org/mod/test/integration']);
      assert.equal(report.branchPackages.find(pkg => pkg.package.endsWith('/test/integration')).skipped, 'No non-test Go files');
      assert.deepEqual(report.statements, { covered: mode === 'below-statements' ? 8 : 9, total: mode === 'below-statements' ? 9 : 10 });
      assert.deepEqual(report.branches, mode === 'missing-branches' ? { covered: 0, total: 0 }
        : { covered: mode === 'below-branches' ? 6 : 10, total: 12 });
      assert.ok(readFileSync(join(out, 'summary.md'), 'utf8').includes('90%'));
      assert.ok(readFileSync(join(out, 'provenance.json'), 'utf8').includes('fixture-commit'));
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

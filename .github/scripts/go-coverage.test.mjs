import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { absoluteReplacements, branchCounts, isTestCode, meetsTarget, statementCounts } from './go-coverage.mjs';

test('relative replace paths become absolute; module and versioned targets stay', () => {
  const gomod = [
    'module example.org/mod',
    '',
    'replace example.org/a => ../a',
    'replace example.org/b v1.0.0 => ./vendor/b // local',
    'replace example.org/c => example.org/fork v1.2.3',
    'replace (',
    '\texample.org/d => ../d',
    '\texample.org/e => example.org/e v0.1.0',
    ')',
    'require example.org/f v1.0.0',
    '',
  ].join('\n');
  assert.equal(absoluteReplacements(gomod, '/repo/mod'), [
    'module example.org/mod',
    '',
    'replace example.org/a => /repo/a',
    'replace example.org/b v1.0.0 => /repo/mod/vendor/b // local',
    'replace example.org/c => example.org/fork v1.2.3',
    'replace (',
    '\texample.org/d => /repo/d',
    '\texample.org/e => example.org/e v0.1.0',
    ')',
    'require example.org/f v1.0.0',
    '',
  ].join('\n'));
  assert.equal(absoluteReplacements('replace example.org/a => ../a\n', '/my repo/mod'),
    'replace example.org/a => "/my repo/a"\n');
  assert.equal(absoluteReplacements('module example.org/mod\nrequire example.org/a v1.0.0\n', '/repo/mod'), null);
});

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

test('test trees and parent-named test helpers are test code', () => {
  for (const name of ['example.org/mod/test/utils', 'example.org/mod/test', 'example.org/mod/pkg/wasmbus/wasmbustest']) {
    assert.equal(isTestCode(name, 'example.org/mod'), true, name);
  }
  for (const name of ['example.org/mod', 'example.org/mod/pkg/wasmbus', 'example.org/mod/testdata', 'example.org/mod/pkg/latest', 'example.org/mod/pkg/contest']) {
    assert.equal(isTestCode(name, 'example.org/mod'), false, name);
  }
});

test('the module path does not make its packages test code', () => {
  assert.equal(isTestCode('example.org/test/mod', 'example.org/test/mod'), false);
  assert.equal(isTestCode('example.org/test/mod/pkg', 'example.org/test/mod'), false);
  assert.equal(isTestCode('example.org/test/mod/test/utils', 'example.org/test/mod'), true);
  assert.equal(isTestCode('example.org/mod/modtest', 'example.org/mod'), true);
  assert.equal(isTestCode('example.org/modtest', 'example.org/modtest'), false);
  assert.throws(() => isTestCode('example.org/other/pkg', 'example.org/mod'));
});

test('target comparisons use exact counts instead of rounded percentages', () => {
  assert.equal(meetsTarget({ covered: 89999, total: 100000 }, 90), false);
  assert.equal(meetsTarget({ covered: 90, total: 100 }, 90), true);
  assert.equal(meetsTarget({ covered: 80, total: 100 }, 80), true);
  assert.equal(meetsTarget({ covered: 0, total: 0 }, 80), false);
});

test('collector tracks targets without enforcement and rejects failed or incomplete runs', () => {
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
if ((tool === 'gobco' && args[0] !== '-version') || (tool === 'go' && args[0] === 'test')) {
  const modfile = (process.env.GOFLAGS ?? '').split(' ').find(flag => flag.startsWith('-modfile='))?.slice('-modfile='.length);
  // The fixture root contains spaces, so the absolute replacement is quoted.
  if (!modfile || !/example\\.org\\/dep => "\\/.* .*\\/dep"/.test(fs.readFileSync(modfile, 'utf8'))) {
    throw new Error('relative replace not made absolute');
  }
}
if (tool === 'git') {
  if (args[0] === 'rev-parse') console.log('fixture-commit');
} else if (tool === 'go') {
  if (args[0] === 'version') console.log('go fixture');
  else if (args[0] === 'list') {
    for (const pkg of ['', '/untested', '/onlytests', '/fake/faketest', '/test/integration', '/test/e2e']) {
      console.log('example.org/mod' + pkg + '\\t' + process.cwd() + pkg + '\\t' + !(pkg.startsWith('/test/') || pkg === '/onlytests') + '\\texample.org/mod');
    }
  } else if (args[0] === 'test') {
    if (process.env.GOWORK !== 'off') throw new Error('workspace enabled');
    if (!args.includes('-covermode=atomic') || !args.includes('-count=1')) throw new Error('wrong coverage flags');
    if (!args.includes('-coverpkg=example.org/mod,example.org/mod/untested,example.org/mod/onlytests')) throw new Error('wrong denominator');
    if (!args.includes('example.org/mod/test/integration') || !args.includes('example.org/mod/fake/faketest')) throw new Error('test packages not run');
    if (args.includes('example.org/mod/test/e2e')) throw new Error('e2e run');
    fs.writeFileSync(args.find(a => a.startsWith('-coverprofile=')).split('=')[1],
      'mode: atomic\\nexample.org/mod/a.go:1.1,2.2 ' + (mode === 'below-statements' ? 8 : 9) + ' 1\\nexample.org/mod/a.go:1.1,2.2 ' + (mode === 'below-statements' ? 8 : 9) + ' 0\\nexample.org/mod/untested/b.go:1.1,2.2 1 0\\n');
    if (mode === 'empty-statements') fs.writeFileSync(args.find(a => a.startsWith('-coverprofile=')).split('=')[1], 'mode: atomic\\na.go:1.1,2.2 0 0\\n');
    process.exit(mode === 'test-failure' ? 1 : 0);
  } else if (args.includes('-func')) console.log('total: 90.0%');
  else if (args.includes('-html')) fs.writeFileSync(args[args.indexOf('-o') + 1], '<html></html>');
  else throw new Error('unexpected go command');
} else if (tool === 'gobco') {
  if (args[0] === '-version') console.log('gobco fixture');
  else {
    if (args.at(-1).endsWith('/onlytests') || args.at(-1).endsWith('/test/integration') || args.at(-1).endsWith('/fake/faketest')) throw new Error('instrumented test code');
    if (!args.includes('-branch')) throw new Error('condition coverage requested');
    if (mode !== 'missing-branches') {
      const count = mode === 'empty-branches' ? 0 : args.at(-1).endsWith('/untested') ? 1 : 5;
      fs.writeFileSync(args[args.indexOf('-stats') + 1], JSON.stringify(Array.from({ length: count }, (_, i) =>
        ({ Start: 'a.go:' + i + ':1', Code: 'x', TrueCount: 1, FalseCount: i === 0 || mode === 'below-branches' ? 0 : 1 }))));
    }
    process.exit(mode === 'branch-failure' ? 1 : 0);
  }
} else throw new Error('unexpected tool');
`;
    writeFileSync(join(module, 'go.mod'), 'module example.org/mod\n\nreplace example.org/dep => ../dep\n');
    for (const tool of ['go', 'gobco', 'git']) {
      writeFileSync(join(bin, tool), fake);
      chmodSync(join(bin, tool), 0o755);
    }
    for (const mode of ['passing', 'below-statements', 'below-branches', 'test-failure', 'branch-failure', 'missing-branches', 'empty-statements', 'empty-branches']) {
      const result = spawnSync(process.execPath, [script, 'runtime-gateway'], {
        env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, GOBCO: join(bin, 'gobco'), FIXTURE_MODE: mode, GITHUB_STEP_SUMMARY: join(root, 'summary.md') },
        encoding: 'utf8',
      });
      const complete = ['passing', 'below-statements', 'below-branches'].includes(mode);
      assert.equal(result.status, complete ? 0 : 1, result.stderr);
      const report = JSON.parse(readFileSync(join(out, 'coverage.json'), 'utf8'));
      assert.equal(report.passed, complete);
      assert.equal(report.complete, complete);
      assert.equal(report.targetsMet, complete ? mode === 'passing' : null);
      assert.equal(report.targetsEnforced, false);
      assert.deepEqual(report.targets, { statements: 90, branches: 80 });
      assert.deepEqual(report.packages, ['example.org/mod', 'example.org/mod/untested', 'example.org/mod/onlytests']);
      assert.deepEqual(report.testPackages, ['example.org/mod/fake/faketest', 'example.org/mod/test/integration']);
      assert.ok(report.branchPackages.every(pkg => report.packages.includes(pkg.package)), 'branch-measured test code');
      assert.equal(report.branchPackages.find(pkg => pkg.package.endsWith('/onlytests')).skipped, 'No non-test Go files');
      assert.deepEqual(report.statements, mode === 'empty-statements' ? { covered: 0, total: 0 }
        : { covered: mode === 'below-statements' ? 8 : 9, total: mode === 'below-statements' ? 9 : 10 });
      assert.deepEqual(report.branches, ['missing-branches', 'empty-branches'].includes(mode) ? { covered: 0, total: 0 }
        : { covered: mode === 'below-branches' ? 6 : 10, total: 12 });
      const summary = readFileSync(join(out, 'summary.md'), 'utf8');
      assert.ok(summary.includes('90%'));
      assert.ok(summary.includes('Coverage percentages do not affect CI status.'));
      assert.ok(summary.includes(`Gold targets (informational): ${complete ? mode === 'passing' ? 'met' : 'below target' : 'not assessed'}`));
      assert.ok(readFileSync(join(out, 'provenance.json'), 'utf8').includes('fixture-commit'));
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

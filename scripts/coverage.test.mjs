import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import { test } from 'node:test';

function fixture(t) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'coverage runner-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, 'scripts'));
  mkdirSync(join(root, 'bin'));
  copyFileSync(new URL('./coverage.mjs', import.meta.url), join(root, 'scripts/coverage.mjs'));
  const stub = `#!/usr/bin/env node
const { appendFileSync, mkdirSync, writeFileSync } = require('node:fs');
const { basename } = require('node:path');
const args = process.argv.slice(2);
const command = basename(process.argv[1]);
appendFileSync(process.env.CALL_LOG, JSON.stringify({ command, args, cwd: process.cwd(), debug: process.env.CARGO_PROFILE_TEST_DEBUG, incremental: process.env.CARGO_INCREMENTAL }) + '\\n');
if (command === 'git' && args[0] === 'rev-parse') console.log('test-commit');
if (command === 'rustc') console.log('test-toolchain');
if (command !== 'cargo') process.exit(0);
if (args[1] === '--version') console.log('cargo-llvm-cov test-version');
if (args[1] === 'clean') process.exit(Number(process.env.CLEAN_EXIT ?? 0));
if (args[0] === 'xtask') {
  if (process.env.BLOCK_TEST_LOG) mkdirSync('target/coverage/tests.txt');
  process.exit(Number(process.env.FIXTURE_EXIT ?? 0));
}
if (args.includes('--no-report')) {
  console.log('test stdout');
  console.error('test stderr');
  if (process.env.TEST_SIGNAL) process.kill(process.pid, process.env.TEST_SIGNAL);
  else process.exit(Number(process.env.TEST_EXIT ?? 0));
}
if (args[1] === 'report') {
  console.log('report stdout');
  console.error('report stderr');
  if (args.includes('--fail-under-lines')) process.exit(Number(process.env.THRESHOLD_EXIT ?? 0));
  if (process.env.REPORT_EXIT) process.exit(Number(process.env.REPORT_EXIT));
  const pathIndex = args.indexOf('--output-path');
  if (pathIndex !== -1) writeFileSync(args[pathIndex + 1], 'report');
  const dirIndex = args.indexOf('--output-dir');
  if (dirIndex !== -1) {
    mkdirSync(args[dirIndex + 1] + '/html', { recursive: true });
    writeFileSync(args[dirIndex + 1] + '/html/index.html', 'report');
  }
}
`;
  for (const command of ['git', 'rustc', 'cargo']) {
    writeFileSync(join(root, 'bin', command), stub, { mode: 0o755 });
  }
  const report = join(root, 'target/coverage');
  const log = join(root, 'calls.jsonl');
  return {
    root,
    report,
    calls: () => existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').map(JSON.parse) : [],
    run: (mode, env = {}) => spawnSync(process.execPath, [join(root, 'scripts/coverage.mjs'), ...(mode ? [mode] : [])], {
      cwd: tmpdir(),
      encoding: 'utf8',
      env: { ...process.env, PATH: `${join(root, 'bin')}${delimiter}${process.env.PATH}`, CALL_LOG: log, ...env },
    }),
  };
}

function expectStatus(result, status) {
  assert.equal(result.status, status, result.stderr);
}

test('all runs from the repository root and produces complete evidence', (t) => {
  const f = fixture(t);
  expectStatus(f.run(), 0);
  for (const file of ['tests-passed', 'tests.txt', 'summary.txt', 'coverage.json', 'lcov.info', 'html/index.html', 'threshold.txt']) {
    assert.ok(existsSync(join(f.report, file)), file);
  }
  const calls = f.calls();
  assert.ok(calls.every(c => c.cwd === f.root && c.debug === 'line-tables-only' && c.incremental === '0'));
  const build = calls.findIndex(c => c.args[0] === 'xtask');
  const tests = calls.findIndex(c => c.args.includes('--no-report'));
  assert.ok(build < tests);
  assert.ok(calls[tests].args.includes('--include-ignored'));
  assert.ok(calls[tests].args.includes('--no-fail-fast'));
  assert.equal(calls[tests].args[calls[tests].args.indexOf('--features') + 1], 'wash-runtime/wasi-tls,wash/host-component-plugins');
  const threshold = calls.find(c => c.args.includes('--fail-under-lines'));
  assert.equal(threshold.args[threshold.args.indexOf('--fail-under-lines') + 1], '80');
  assert.equal(threshold.args[threshold.args.indexOf('--fail-under-regions') + 1], '80');
  assert.ok(calls.filter(c => c.args[1] === 'report').every(c => c.args.includes(String.raw`[/\\]washlet[/\\]generated[/\\]`)));
  const provenance = readFileSync(join(f.report, 'provenance.txt'), 'utf8');
  assert.match(provenance, /test-commit/);
  assert.match(provenance, /Test exit status: 0/);
});

test('failed tests preserve diagnostics without passing evidence', (t) => {
  const f = fixture(t);
  expectStatus(f.run('all', { TEST_EXIT: '7' }), 7);
  assert.ok(!existsSync(join(f.report, 'tests-passed')));
  assert.ok(existsSync(join(f.report, 'coverage.json')));
  assert.match(readFileSync(join(f.report, 'tests.txt'), 'utf8'), /test stdout/);
  assert.match(readFileSync(join(f.report, 'tests.txt'), 'utf8'), /test stderr/);
  assert.match(readFileSync(join(f.report, 'provenance.txt'), 'utf8'), /Test exit status: 7/);
  assert.ok(!f.calls().some(c => c.args.includes('--fail-under-lines')));
});

test('fixture failures clear stale evidence and prevent a threshold pass', (t) => {
  const f = fixture(t);
  mkdirSync(f.report, { recursive: true });
  writeFileSync(join(f.report, 'tests-passed'), 'stale');
  writeFileSync(join(f.report, 'coverage.json'), 'stale');
  expectStatus(f.run('test', { FIXTURE_EXIT: '23' }), 23);
  assert.ok(!existsSync(join(f.report, 'tests-passed')));
  assert.ok(!existsSync(join(f.report, 'coverage.json')));
  assert.ok(!f.calls().some(c => c.args.includes('--no-report')));
  expectStatus(f.run('check'), 1);
  assert.ok(!f.calls().some(c => c.args.includes('--fail-under-lines')));
});

test('threshold failures retain both output streams and propagate their status', (t) => {
  const f = fixture(t);
  expectStatus(f.run('all', { THRESHOLD_EXIT: '9' }), 9);
  const log = readFileSync(join(f.report, 'threshold.txt'), 'utf8');
  assert.match(log, /report stdout/);
  assert.match(log, /report stderr/);
});

test('report failures propagate without checking thresholds', (t) => {
  const f = fixture(t);
  expectStatus(f.run('all', { REPORT_EXIT: '12' }), 12);
  assert.ok(!f.calls().some(c => c.args.includes('--fail-under-lines')));
});

test('terminated tests never create passing evidence', (t) => {
  const f = fixture(t);
  expectStatus(f.run('test', { TEST_SIGNAL: 'SIGTERM' }), 143);
  assert.ok(!existsSync(join(f.report, 'tests-passed')));
});

test('unknown modes fail before invoking tools', (t) => {
  const f = fixture(t);
  expectStatus(f.run('unknown'), 2);
  assert.deepEqual(f.calls(), []);
});

test('unwritable test logs cannot produce passing evidence', (t) => {
  const f = fixture(t);
  expectStatus(f.run('test', { BLOCK_TEST_LOG: '1' }), 1);
  assert.ok(!existsSync(join(f.report, 'tests-passed')));
});

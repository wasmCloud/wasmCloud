import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const scripts = dirname(fileURLToPath(import.meta.url));
const binaries = ['wash-x86_64-unknown-linux-gnu', 'runtime-operator', 'runtime-gateway'];

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'release-binaries-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const dir of ['first', 'second', 'bin', 'checkout with spaces/.github/scripts',
    'checkout with spaces/runtime-operator', 'checkout with spaces/runtime-gateway']) {
    mkdirSync(join(root, dir), { recursive: true });
  }
  const env = { ...process.env, PATH: `${join(root, 'bin')}:${process.env.PATH}`, MOCK_ROOT: root };
  const executable = (name, body) => {
    const path = join(root, 'bin', name);
    writeFileSync(path, `#!${process.execPath}\n${body}\n`);
    chmodSync(path, 0o755);
  };
  executable('diffoscope', `
    const fs = require('node:fs');
    const args = process.argv.slice(2);
    fs.appendFileSync(process.env.MOCK_ROOT + '/diagnostics', JSON.stringify(args) + '\\n');
    fs.writeFileSync(args[1], 'different bytes');
    fs.writeFileSync(args[3], '<html>different bytes</html>');
    process.exit(Number(process.env.DIFFOSCOPE_STATUS || 1));
  `);
  executable('sudo', `
    require('node:fs').appendFileSync(process.env.MOCK_ROOT + '/install', JSON.stringify(process.argv.slice(2)) + '\\n');
    process.exit(Number(process.env.INSTALL_STATUS || 0));
  `);
  const fill = (different = false) => {
    for (const name of binaries) {
      writeFileSync(join(root, 'first', name), Buffer.from([0, 255, 1, 2]));
      writeFileSync(join(root, 'second', name), Buffer.from([0, 255, 1, different ? 3 : 2]));
    }
  };
  const compare = (...args) => spawnSync(process.execPath, [join(scripts, 'compare-release-binaries.mjs'),
    join(root, 'first'), join(root, 'second'), join(root, 'reports'), ...args], { env, encoding: 'utf8' });
  return { root, env, executable, fill, compare };
}

test('matching binary bytes need neither diffoscope nor a package install', (t) => {
  const f = fixture(t);
  f.fill();
  const result = f.compare('--install-diffoscope');
  assert.equal(result.status, 0, result.stderr);
  assert.equal(existsSync(join(f.root, 'diagnostics')), false);
  assert.equal(existsSync(join(f.root, 'install')), false);
  const digest = createHash('sha256').update(Buffer.from([0, 255, 1, 2])).digest('hex');
  assert.equal(result.stdout.split(digest).length - 1, 6);
});

test('every mismatched binary gets reports, and diffoscope installs once', (t) => {
  const f = fixture(t);
  f.fill(true);
  assert.equal(f.compare('--install-diffoscope').status, 1);
  assert.equal(readFileSync(join(f.root, 'install'), 'utf8').trim().split('\n').length, 2);
  assert.equal(readFileSync(join(f.root, 'diagnostics'), 'utf8').trim().split('\n').length, 3);
  for (const name of binaries) {
    assert.equal(existsSync(join(f.root, 'reports', `${name}.diff.html`)), true);
    assert.equal(existsSync(join(f.root, 'reports', `${name}.sha256`)), true);
  }
});

test('diagnostic errors preserve mismatch failure and do not stop other comparisons', (t) => {
  const f = fixture(t);
  f.fill(true);
  f.env.DIFFOSCOPE_STATUS = '2';
  assert.equal(f.compare().status, 1);
  assert.equal(readFileSync(join(f.root, 'diagnostics'), 'utf8').trim().split('\n').length, 3);
  for (const name of binaries) {
    assert.match(readFileSync(join(f.root, 'reports', `${name}.diffoscope.log`), 'utf8'), /diffoscope failed/);
  }
});

test('missing and empty binaries fail while remaining binaries are checked', (t) => {
  const f = fixture(t);
  f.fill();
  rmSync(join(f.root, 'second', binaries[0]));
  writeFileSync(join(f.root, 'second', binaries[1]), '');
  assert.equal(f.compare().status, 1);
  assert.equal(existsSync(join(f.root, 'reports', `${binaries[0]}.error.txt`)), true);
  assert.equal(existsSync(join(f.root, 'reports', `${binaries[1]}.error.txt`)), true);
  assert.equal(existsSync(join(f.root, 'reports', `${binaries[2]}.sha256`)), true);
});

test('wash-only release comparison does not require the Go artifacts', (t) => {
  const f = fixture(t);
  f.fill();
  for (const name of binaries.slice(1)) rmSync(join(f.root, 'second', name));
  assert.equal(f.compare(binaries[0]).status, 0);
  assert.equal(f.compare('../unexpected').status, 2);
});

test('package installation failure still records every mismatched hash', (t) => {
  const f = fixture(t);
  f.fill(true);
  f.env.INSTALL_STATUS = '100';
  assert.equal(f.compare('--install-diffoscope').status, 1);
  for (const name of binaries) {
    assert.equal(existsSync(join(f.root, 'reports', `${name}.sha256`)), true);
    assert.match(readFileSync(join(f.root, 'reports', `${name}.diffoscope.log`), 'utf8'), /Could not install/);
  }
});

test('a failed compiler stops the build and returns its exit status', (t) => {
  const f = fixture(t);
  const checkout = join(f.root, 'checkout with spaces');
  const build = join(checkout, '.github/scripts/build-release-binary.mjs');
  copyFileSync(join(scripts, 'build-release-binary.mjs'), build);
  f.executable('git', 'process.stdout.write("1234567890\\n");');
  f.executable('cargo', `process.exit(42);`);
  f.executable('go', `require('node:fs').writeFileSync(process.env.MOCK_ROOT + '/go-ran', '');`);
  const result = spawnSync(process.execPath, [build, 'all', join(f.root, 'output')], {
    cwd: tmpdir(), env: { ...f.env, CARGO_HOME: join(f.root, 'cargo home') }, encoding: 'utf8',
  });
  assert.equal(result.status, 42, result.stderr);
  assert.equal(existsSync(join(f.root, 'go-ran')), false);
  assert.equal(existsSync(join(f.root, 'output/build-environment.json')), false);
});

test('pin reader rejects mismatched Go versions and reports exact pins', (t) => {
  const f = fixture(t);
  const checkout = join(f.root, 'checkout with spaces');
  const script = join(checkout, '.github/scripts/release-build-pins.mjs');
  copyFileSync(join(scripts, 'release-build-pins.mjs'), script);
  writeFileSync(join(checkout, 'rust-toolchain.toml'), '[toolchain]\nchannel = "1.96.0"\n');
  for (const name of ['runtime-operator', 'runtime-gateway']) {
    writeFileSync(join(checkout, name, 'Dockerfile'), 'FROM golang:1.27.2 AS builder\n');
  }
  const output = join(f.root, 'github-output');
  const run = () => spawnSync(process.execPath, [script], {
    cwd: tmpdir(), env: { ...f.env, GITHUB_OUTPUT: output }, encoding: 'utf8',
  });
  assert.equal(run().status, 0);
  assert.equal(readFileSync(output, 'utf8'), 'rust=1.96.0\ngo=1.27.2\n');
  writeFileSync(join(checkout, 'runtime-gateway/Dockerfile'), 'FROM golang:1.27.3 AS builder\n');
  assert.equal(run().status, 1);
  assert.equal(readFileSync(output, 'utf8'), 'rust=1.96.0\ngo=1.27.2\n');
});

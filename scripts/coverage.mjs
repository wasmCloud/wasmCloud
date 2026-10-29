#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { appendFileSync, createWriteStream, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { constants } from 'node:os';
import { fileURLToPath } from 'node:url';

process.chdir(fileURLToPath(new URL('..', import.meta.url)));
Object.assign(process.env, {
  CARGO_PROFILE_DEV_DEBUG: 'line-tables-only',
  CARGO_PROFILE_TEST_DEBUG: 'line-tables-only',
  CARGO_INCREMENTAL: '0',
});

const features = 'wash-runtime/wasi-tls,wash/host-component-plugins';
const reportDir = 'target/coverage';
// Generated protobuf bindings have no maintained application logic.
const reportArgs = ['--ignore-filename-regex', String.raw`[/\\]washlet[/\\]generated[/\\]`];

class CommandFailure extends Error {
  constructor(message, exitCode = 1) {
    super(message);
    this.exitCode = exitCode;
  }
}

function run(command, args, { log, append = false, echo = true } = {}) {
  return new Promise((resolve, reject) => {
    const output = log ? createWriteStream(log, { flags: append ? 'a' : 'w' }) : undefined;
    const child = spawn(command, args, { stdio: log ? ['inherit', 'pipe', 'pipe'] : 'inherit' });
    let failure;
    output?.on('error', (error) => {
      failure = error;
      child.kill();
    });
    if (output) {
      for (const [source, destination] of [[child.stdout, process.stdout], [child.stderr, process.stderr]]) {
        source.pipe(output, { end: false });
        if (echo) source.pipe(destination, { end: false });
      }
    }
    child.once('error', (error) => {
      failure = new CommandFailure(error.message, error.code === 'ENOENT' ? 127 : 1);
    });
    child.once('close', (code, signal) => {
      const status = code ?? (128 + (constants.signals[signal] ?? 0));
      const finish = (error) => {
        failure ??= error;
        if (failure) reject(failure);
        else if (status !== 0) reject(new CommandFailure(`${command} failed (exit ${status})`, status));
        else resolve();
      };
      if (output) output.end(finish);
      else finish();
    });
  });
}

async function collect() {
  rmSync(reportDir, { recursive: true, force: true });
  mkdirSync(reportDir, { recursive: true });
  const provenance = `${reportDir}/provenance.txt`;
  writeFileSync(provenance, `Collected: ${new Date().toISOString()}\n`);
  for (const [command, args] of [
    ['git', ['rev-parse', 'HEAD']],
    ['git', ['status', '--short']],
    ['rustc', ['-vV']],
    ['cargo', ['llvm-cov', '--version']],
  ]) {
    await run(command, args, { log: provenance, append: true, echo: false });
  }
  appendFileSync(provenance, `Features: ${features}\nTests: workspace unit and integration tests, including ignored tests\nScope: native Rust workspace; generated protobuf bindings excluded\n`);
  if (process.env.GITHUB_RUN_ID) {
    appendFileSync(provenance, `Run: ${process.env.GITHUB_SERVER_URL}/${process.env.GITHUB_REPOSITORY}/actions/runs/${process.env.GITHUB_RUN_ID}\n`);
  }
  await run('cargo', ['llvm-cov', 'clean', '--workspace']);
  // Build guests before cargo-llvm-cov enables native instrumentation.
  await run('cargo', ['xtask', 'build-fixtures']);
  let status = 0;
  try {
    await run('cargo', ['llvm-cov', '--locked', '--workspace', '--features', features, '--no-report', '--no-fail-fast', '--', '--include-ignored'], { log: `${reportDir}/tests.txt` });
  } catch (error) {
    status = error.exitCode ?? 1;
    throw error;
  } finally {
    appendFileSync(provenance, `Test exit status: ${status}\n`);
  }
  writeFileSync(`${reportDir}/tests-passed`, '');
}

async function report() {
  mkdirSync(reportDir, { recursive: true });
  const args = ['llvm-cov', 'report', ...reportArgs];
  await run('cargo', args, { log: `${reportDir}/summary.txt` });
  await run('cargo', [...args, '--json', '--summary-only', '--output-path', `${reportDir}/coverage.json`]);
  await run('cargo', [...args, '--lcov', '--output-path', `${reportDir}/lcov.info`]);
  await run('cargo', [...args, '--html', '--output-dir', reportDir]);
}

async function check() {
  if (!existsSync(`${reportDir}/tests-passed`)) {
    throw new CommandFailure('Coverage evidence requires a successful full test run.');
  }
  await run('cargo', ['llvm-cov', 'report', ...reportArgs, '--fail-under-lines', '80', '--fail-under-regions', '80'], { log: `${reportDir}/threshold.txt` });
}

async function main() {
  switch (process.argv[2] ?? 'all') {
    case 'test': return collect();
    case 'report': return report();
    case 'check': return check();
    case 'all': {
      let testError;
      try { await collect(); } catch (error) { testError = error; }
      // Keep diagnostics when tests or the threshold check fail.
      await report();
      if (testError) throw testError;
      return check();
    }
    default: throw new CommandFailure('Usage: node scripts/coverage.mjs [all|test|report|check]', 2);
  }
}

try {
  await main();
} catch (error) {
  console.error(error.message);
  process.exitCode = error.exitCode ?? 1;
}

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const script = fileURLToPath(new URL('../actions/setup-rust/retry.sh', import.meta.url));

function runDownload(failures) {
  const dir = mkdtempSync(join(tmpdir(), 'retry-download-'));
  try {
    const result = spawnSync('bash', ['-c', `
      set -euo pipefail
      source "$RETRY_SCRIPT"
      attempts=0
      download() {
        attempts=$((attempts + 1))
        printf '%s\\n' "$attempts" >> "$ATTEMPT_LOG"
        printf '%s\\n' "$@" >> "$ARGUMENT_LOG"
        if [ "$attempts" -le "$FAILURES" ]; then return 23; fi
      }
      sleep() { printf '%s\\n' "$1" >> "$SLEEP_LOG"; }
      retry_download download 'toolchain with spaces' --component 'clippy,rustfmt'
      echo completed
    `], {
      encoding: 'utf8',
      env: {
        ...process.env,
        RETRY_SCRIPT: script,
        FAILURES: String(failures),
        ATTEMPT_LOG: join(dir, 'attempts'),
        ARGUMENT_LOG: join(dir, 'arguments'),
        SLEEP_LOG: join(dir, 'sleeps'),
      },
    });
    return {
      ...result,
      attempts: readFileSync(join(dir, 'attempts'), 'utf8').trim().split('\n'),
      arguments: readFileSync(join(dir, 'arguments'), 'utf8').trim().split('\n'),
      sleeps: failures ? readFileSync(join(dir, 'sleeps'), 'utf8').trim().split('\n') : [],
    };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('successful downloads run once without sleeping', () => {
  const result = runDownload(0);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.attempts, ['1']);
  assert.deepEqual(result.sleeps, []);
});

test('transient downloads recover with bounded backoff and intact arguments', () => {
  const result = runDownload(2);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.attempts, ['1', '2', '3']);
  assert.deepEqual(result.sleeps, ['5', '10']);
  assert.deepEqual(result.arguments, Array(3).fill(['toolchain with spaces', '--component', 'clippy,rustfmt']).flat());
  assert.match(result.stdout, /completed/);
});

test('exhausted downloads preserve the failure and stop the setup script', () => {
  const result = runDownload(3);
  assert.equal(result.status, 23, result.stderr);
  assert.deepEqual(result.attempts, ['1', '2', '3']);
  assert.deepEqual(result.sleeps, ['5', '10']);
  assert.doesNotMatch(result.stdout, /completed/);
});

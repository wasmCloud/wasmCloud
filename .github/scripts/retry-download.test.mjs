import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const script = fileURLToPath(new URL('../actions/setup-rust/retry.sh', import.meta.url));

// The download fails `failures` times, printing `message` to stderr each time.
function runDownload(failures, message = 'error: could not download file') {
  const dir = mkdtempSync(join(tmpdir(), 'retry-download-'));
  const lines = (name) => {
    const path = join(dir, name);
    return existsSync(path) ? readFileSync(path, 'utf8').trim().split('\n') : [];
  };
  try {
    const result = spawnSync('bash', ['-c', `
      set -euo pipefail
      source "$RETRY_SCRIPT"
      download() {
        # Counted on disk: the command under retry does not run in this shell.
        printf 'attempt\\n' >> "$ATTEMPT_LOG"
        printf '%s\\n' "$@" >> "$ARGUMENT_LOG"
        echo progress
        if [ "$(wc -l < "$ATTEMPT_LOG")" -le "$FAILURES" ]; then
          echo "$MESSAGE" >&2
          return 23
        fi
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
        MESSAGE: message,
        ATTEMPT_LOG: join(dir, 'attempts'),
        ARGUMENT_LOG: join(dir, 'arguments'),
        SLEEP_LOG: join(dir, 'sleeps'),
      },
    });
    return {
      ...result,
      attempts: lines('attempts').length,
      arguments: lines('arguments'),
      sleeps: lines('sleeps'),
    };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test('successful downloads run once without sleeping', () => {
  const result = runDownload(0);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.attempts, 1);
  assert.deepEqual(result.sleeps, []);
  assert.equal(result.stdout, 'progress\ncompleted\n');
  assert.equal(result.stderr, '');
});

test('transient downloads recover with bounded backoff and intact arguments', () => {
  const result = runDownload(2);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.attempts, 3);
  assert.deepEqual(result.sleeps, ['5', '10']);
  assert.deepEqual(result.arguments, Array(3).fill(['toolchain with spaces', '--component', 'clippy,rustfmt']).flat());
  assert.match(result.stdout, /completed/);
  assert.equal(result.stderr.match(/could not download file/g).length, 2);
  assert.equal(result.stderr.match(/::warning::/g).length, 2);
});

test('exhausted downloads preserve the failure and stop the setup script', () => {
  const result = runDownload(3);
  assert.equal(result.status, 23, result.stderr);
  assert.equal(result.attempts, 3);
  assert.deepEqual(result.sleeps, ['5', '10']);
  assert.doesNotMatch(result.stdout, /completed/);
});

// The messages rustup 1.29 prints for requests no retry can satisfy.
for (const [request, message] of [
  ['unknown target', "error: toolchain 'stable-x86_64-unknown-linux-gnu' does not support target 'wasm32-wasip9'"],
  ['unknown component', "error: toolchain 'stable-x86_64-unknown-linux-gnu' does not contain component 'clipy' for target 'x86_64-unknown-linux-gnu'"],
  ['component the channel lacks', "error: component 'rust-std' for target 'wasm32-wasip9' is unavailable for download for channel 'stable-x86_64-unknown-linux-gnu'"],
  ['invalid toolchain name', "error: invalid value 'sta ble' for '[TOOLCHAIN]...': invalid toolchain name: 'sta ble'"],
]) {
  test(`a request that cannot succeed fails at once: ${request}`, () => {
    const result = runDownload(3, message);
    assert.equal(result.status, 23, result.stderr);
    assert.equal(result.attempts, 1);
    assert.deepEqual(result.sleeps, []);
    assert.equal(result.stderr, `${message}\n`);
    assert.doesNotMatch(result.stdout, /completed/);
  });
}

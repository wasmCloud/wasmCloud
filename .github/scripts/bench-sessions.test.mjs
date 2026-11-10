import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import {
  ancestry,
  findLeftovers,
  formatLeftovers,
  loginctlSession,
  parentPid,
  parseSessionProps,
  scanSessionProcesses,
  sessionOfCgroup,
  settledLeftovers,
} from './bench-sessions.mjs';

const SCOPE = (uid, id) => `0::/user.slice/user-${uid}.slice/session-${id}.scope\n`;

function fakeProc(t, procs) {
  const root = mkdtempSync(join(tmpdir(), 'bench-sessions-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const { pid, ppid = 1, uid = 0, cgroup, argv = [], comm = 'cmd', exe, cwd } of procs) {
    const dir = join(root, String(pid));
    mkdirSync(dir);
    writeFileSync(join(dir, 'cgroup'), cgroup);
    writeFileSync(join(dir, 'status'), `Name:\t${comm}\nUid:\t${uid}\t${uid}\t${uid}\t${uid}\n`);
    writeFileSync(join(dir, 'cmdline'), argv.map((a) => `${a}\0`).join(''));
    writeFileSync(join(dir, 'stat'), `${pid} (${comm}) S ${ppid} ${pid} ${pid} 0 -1\n`);
    if (exe) symlinkSync(exe, join(dir, 'exe'));
    if (cwd) symlinkSync(cwd, join(dir, 'cwd'));
  }
  mkdirSync(join(root, 'self'));
  return root;
}

test('reads the session id from a session scope cgroup only', () => {
  assert.equal(sessionOfCgroup(SCOPE(0, '1234')), '1234');
  assert.equal(sessionOfCgroup(`12:pids:/\n1:name=systemd:/user.slice/user-0.slice/session-c2.scope\n0::/\n`), 'c2');
  assert.equal(sessionOfCgroup('0::/system.slice/actions.runner.wasmCloud-wasmCloud.bench.service\n'), null);
  assert.equal(sessionOfCgroup('0::/user.slice/user-0.slice/user@0.service/init.scope\n'), null);
  assert.equal(sessionOfCgroup('0::/\n'), null);
});

test('reads the parent pid past a command name with spaces and parentheses', () => {
  assert.equal(parentPid('42 (bash) S 7 42 42 0 -1'), 7);
  assert.equal(parentPid('42 (a) b (c) S 9 42 42 0 -1'), 9);
});

test('parses loginctl show-session properties', () => {
  assert.deepEqual(parseSessionProps('Name=root\nState=closing\n'), { Name: 'root', State: 'closing' });
  assert.deepEqual(parseSessionProps(''), {});
});

test('scans only processes in a session scope, with their command lines', (t) => {
  const root = fakeProc(t, [
    { pid: 1582235, cgroup: SCOPE(0, 12), argv: ['bash', '-s'], exe: '/usr/bin/bash', cwd: '/root' },
    { pid: 900, uid: 999, cgroup: '0::/system.slice/actions.runner.x.service\n', argv: ['/opt/actions-runner/bin/Runner.Listener'] },
    { pid: 2, cgroup: '0::/\n' },
    { pid: 77, cgroup: SCOPE(0, 12), argv: [] },
  ]);
  assert.deepEqual(scanSessionProcesses(root), [
    { pid: 77, session: '12', uid: 0, cmdline: '', exe: null, cwd: null },
    { pid: 1582235, session: '12', uid: 0, cmdline: 'bash -s', exe: '/usr/bin/bash', cwd: '/root' },
  ]);
});

test('follows the parent chain to the top', (t) => {
  const root = fakeProc(t, [
    { pid: 30, ppid: 20, cgroup: '0::/\n' },
    { pid: 20, ppid: 10, cgroup: '0::/\n' },
    { pid: 10, ppid: 0, cgroup: '0::/\n' },
  ]);
  assert.deepEqual([...ancestry(30, root)], [30, 20, 10]);
  assert.deepEqual([...ancestry(99, root)], [99]);
});

const sessions = {
  12: { state: 'closing', user: 'root' },
  13: { state: 'active', user: 'root' },
  14: { state: 'online', user: 'bench' },
};
const sessionInfo = (id) => sessions[id] ?? null;

test('reports processes of closing and forgotten sessions only', () => {
  const procs = [
    { pid: 1582235, session: '12', uid: 0, cmdline: 'bash -s' },
    { pid: 1582240, session: '12', uid: 0, cmdline: 'sleep 1' },
    { pid: 500, session: '13', uid: 0, cmdline: '-bash' },
    { pid: 501, session: '14', uid: 999, cmdline: 'tmux' },
    { pid: 600, session: '20', uid: 0, cmdline: 'cargo bench' },
  ];
  assert.deepEqual(findLeftovers(procs, { sessionInfo, runnerUid: 999 }), [
    { session: '12', user: 'root', state: 'closing', procs: procs.slice(0, 2) },
    { session: '20', user: null, state: 'unknown to logind', procs: [procs[4]] },
  ]);
});

test('skips the runner and the pre-flight ancestry', () => {
  const procs = [
    { pid: 900, session: '12', uid: 999, exe: '/opt/actions-runner/bin/Runner.Listener', cwd: '/home/bench' },
    { pid: 901, session: '12', uid: 999, exe: '/opt/actions-runner/externals/node20/bin/node', cwd: '/tmp' },
    // A runner started by hand as `./run.sh`: bash, from the runner directory.
    { pid: 902, session: '12', uid: 999, exe: '/usr/bin/bash', cwd: '/opt/actions-runner' },
    { pid: 910, session: '12', uid: 999, exe: '/usr/bin/node', cwd: '/tmp' },
  ];
  const opts = { sessionInfo, runnerUid: 999, exclude: new Set([910]) };
  assert.deepEqual(findLeftovers(procs, opts), []);
  // Root running something out of the runner directory is not the runner.
  const rootCopy = [{ ...procs[0], uid: 0 }];
  assert.equal(findLeftovers(rootCopy, opts).length, 1);
  // Nor is a lookalike directory.
  const lookalike = [{ ...procs[0], exe: '/opt/actions-runner-old/run.sh' }];
  assert.equal(findLeftovers(lookalike, opts).length, 1);
  // Nor is a process that only names the runner in its argv.
  const spoofed = [{ ...procs[0], exe: '/usr/bin/sleep', cmdline: '/opt/actions-runner/bin/Runner.Listener' }];
  assert.equal(findLeftovers(spoofed, opts).length, 1);
  // Nor a zombie, whose exe and cwd can't be read.
  const zombie = [{ ...procs[0], exe: null, cwd: null }];
  assert.equal(findLeftovers(zombie, opts).length, 1);
});

test('tells a session logind forgot from loginctl itself failing', () => {
  const run = (result) => () => ({ stdout: '', stderr: '', ...result });
  assert.deepEqual(loginctlSession('12', run({ status: 0, stdout: 'State=closing\nName=root\n' })), {
    state: 'closing',
    user: 'root',
  });
  assert.equal(
    loginctlSession('12', run({ status: 1, stderr: "Failed to get path for session '12': No session '12' known\n" })),
    null,
  );
  assert.throws(
    () => loginctlSession('12', run({ status: null, error: new Error('spawnSync loginctl ENOENT') })),
    /loginctl show-session 12 failed: spawnSync loginctl ENOENT/,
  );
  assert.throws(
    () => loginctlSession('12', run({ status: 1, stderr: 'Failed to connect to bus: No such file or directory\n' })),
    /failed: Failed to connect to bus/,
  );
});

test('re-scans before reporting, so a session logind is still stopping passes', () => {
  const closing = [{ pid: 600, session: '12', uid: 0, cmdline: 'cargo bench' }];
  const opts = { sessionInfo, runnerUid: 999 };
  const sleeps = [];
  const sleep = (ms) => sleeps.push(ms);

  const scans = [closing, closing, []];
  assert.deepEqual(settledLeftovers(() => scans.shift(), opts, { delayMs: 10, sleep }), []);
  assert.deepEqual(sleeps, [10, 10]);

  sleeps.length = 0;
  const stuck = settledLeftovers(() => closing, opts, { attempts: 3, delayMs: 10, sleep });
  assert.equal(stuck.length, 1);
  assert.deepEqual(sleeps, [10, 10]);

  sleeps.length = 0;
  assert.deepEqual(settledLeftovers(() => [], opts, { sleep }), []);
  assert.deepEqual(sleeps, []);
});

test('names each leftover pid and command line', () => {
  const message = formatLeftovers([
    {
      session: '12',
      user: 'root',
      state: 'closing',
      procs: [
        { pid: 1582235, cmdline: 'bash -s' },
        { pid: 1582240, cmdline: '' },
      ],
    },
    { session: '20', user: null, state: 'unknown to logind', procs: [{ pid: 600, cmdline: 'x'.repeat(300) }] },
  ]);
  assert.match(message, /session 12 \(root, closing\): PID 1582235: bash -s; PID 1582240: \[no command line\]/);
  assert.match(message, / \| session 20 \(unknown to logind\): PID 600: x{200}…\./);
  assert.match(
    message,
    /Stop them \(sudo loginctl terminate-session 12; sudo systemctl kill --signal=KILL session-20\.scope\)/,
  );
  assert.doesNotMatch(message, /sudo kill /);
  assert.match(message, /systemd-run --unit=<name>/);
  assert.doesNotMatch(message, /\n/);
});

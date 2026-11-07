import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import {
  ancestry,
  findLeftovers,
  formatLeftovers,
  parentPid,
  parseSessionProps,
  scanSessionProcesses,
  sessionOfCgroup,
} from './bench-sessions.mjs';

const SCOPE = (uid, id) => `0::/user.slice/user-${uid}.slice/session-${id}.scope\n`;

function fakeProc(t, procs) {
  const root = mkdtempSync(join(tmpdir(), 'bench-sessions-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const { pid, ppid = 1, uid = 0, cgroup, argv = [], comm = 'cmd' } of procs) {
    const dir = join(root, String(pid));
    mkdirSync(dir);
    writeFileSync(join(dir, 'cgroup'), cgroup);
    writeFileSync(join(dir, 'status'), `Name:\t${comm}\nUid:\t${uid}\t${uid}\t${uid}\t${uid}\n`);
    writeFileSync(join(dir, 'cmdline'), argv.map((a) => `${a}\0`).join(''));
    writeFileSync(join(dir, 'stat'), `${pid} (${comm}) S ${ppid} ${pid} ${pid} 0 -1\n`);
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
    { pid: 1582235, cgroup: SCOPE(0, 12), argv: ['bash', '-s'] },
    { pid: 900, uid: 999, cgroup: '0::/system.slice/actions.runner.x.service\n', argv: ['/opt/actions-runner/bin/Runner.Listener'] },
    { pid: 2, cgroup: '0::/\n' },
    { pid: 77, cgroup: SCOPE(0, 12), argv: [] },
  ]);
  assert.deepEqual(scanSessionProcesses(root), [
    { pid: 77, session: '12', uid: 0, cmdline: '' },
    { pid: 1582235, session: '12', uid: 0, cmdline: 'bash -s' },
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
    { pid: 900, session: '12', uid: 999, cmdline: '/opt/actions-runner/bin/Runner.Listener run' },
    { pid: 901, session: '12', uid: 999, cmdline: '/opt/actions-runner/externals/node20/bin/node' },
    { pid: 910, session: '12', uid: 999, cmdline: 'node bench-preflight.mjs' },
  ];
  const opts = { sessionInfo, runnerUid: 999, exclude: new Set([910]) };
  assert.deepEqual(findLeftovers(procs, opts), []);
  // Root running something out of the runner directory is not the runner.
  const rootCopy = [{ ...procs[0], uid: 0 }];
  assert.equal(findLeftovers(rootCopy, opts).length, 1);
  // Nor is a lookalike directory.
  const lookalike = [{ ...procs[0], cmdline: '/opt/actions-runner-old/run.sh' }];
  assert.equal(findLeftovers(lookalike, opts).length, 1);
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
  assert.match(message, /sudo kill 1582235 1582240 600\)/);
  assert.match(message, /systemd-run --unit=<name>/);
  assert.doesNotMatch(message, /\n/);
});

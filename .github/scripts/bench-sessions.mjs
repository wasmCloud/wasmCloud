// Finds processes that outlived the login session that started them, for
// bench-preflight.mjs. A process started over SSH lives in its session's
// session-<id>.scope cgroup. When the connection drops and logind doesn't
// kill the scope, the session sits in state "closing" for as long as
// anything is left in it, or logind loses track of it entirely.
//
// The actions runner runs from a systemd service, not a login session, so
// its jobs are never in a session scope. Its own processes and this
// script's ancestors are skipped anyway, for a runner someone started by
// hand from an SSH session.

import { spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, readlinkSync } from 'node:fs';
import { join } from 'node:path';

export const RUNNER_DIR = '/opt/actions-runner';
const MAX_CMDLINE = 200;

// The login session id in a /proc/<pid>/cgroup file, or null when the
// process isn't in a session scope.
export function sessionOfCgroup(text) {
  return text.match(/\/session-([A-Za-z0-9]+)\.scope$/m)?.[1] ?? null;
}

// The parent pid in a /proc/<pid>/stat line. The command name is in
// parentheses and may itself contain them, so read past the last ')'.
export function parentPid(stat) {
  const fields = stat.slice(stat.lastIndexOf(')') + 2).split(' ');
  return Number(fields[1]);
}

// `loginctl show-session` output (`Key=Value` lines) as an object.
export function parseSessionProps(text) {
  const props = {};
  for (const line of text.split('\n')) {
    const eq = line.indexOf('=');
    if (eq > 0) props[line.slice(0, eq)] = line.slice(eq + 1);
  }
  return props;
}

function readOrNull(path) {
  try {
    return readFileSync(path, 'utf8');
  } catch {
    return null;
  }
}

function linkOrNull(path) {
  try {
    return readlinkSync(path);
  } catch {
    return null;
  }
}

// Every process in a login session scope, as
// { pid, session, uid, cmdline, exe, cwd }. exe and cwd are null when
// unreadable (another user's process, or a zombie). A process that exits
// mid-scan is skipped.
export function scanSessionProcesses(procRoot = '/proc') {
  const procs = [];
  for (const entry of readdirSync(procRoot)) {
    if (!/^\d+$/.test(entry)) continue;
    const dir = join(procRoot, entry);
    const cgroup = readOrNull(join(dir, 'cgroup'));
    const session = cgroup && sessionOfCgroup(cgroup);
    if (!session) continue;
    const status = readOrNull(join(dir, 'status'));
    const cmdline = readOrNull(join(dir, 'cmdline'));
    if (status === null || cmdline === null) continue;
    const uid = Number(status.match(/^Uid:\s+(\d+)/m)?.[1] ?? NaN);
    procs.push({
      pid: Number(entry),
      session,
      uid,
      cmdline: cmdline.replace(/\0+$/, '').replaceAll('\0', ' '),
      exe: linkOrNull(join(dir, 'exe')),
      cwd: linkOrNull(join(dir, 'cwd')),
    });
  }
  return procs.sort((a, b) => a.pid - b.pid);
}

// `pid` and every ancestor of it up to, not including, pid 0.
export function ancestry(pid, procRoot = '/proc') {
  const pids = new Set();
  while (pid > 0 && !pids.has(pid)) {
    pids.add(pid);
    const stat = readOrNull(join(procRoot, String(pid), 'stat'));
    if (stat === null) break;
    pid = parentPid(stat);
  }
  return pids;
}

// logind's view of a session as { state, user }, or null when logind no
// longer knows it. Throws when loginctl itself fails (missing, D-Bus down,
// timed out), so that isn't mistaken for every session having closed.
export function loginctlSession(id, run = spawnSync) {
  const out = run('loginctl', ['show-session', id, '-p', 'State', '-p', 'Name'], {
    encoding: 'utf8',
    timeout: 10_000,
  });
  if (out.status === 0) {
    const props = parseSessionProps(out.stdout);
    return { state: props.State, user: props.Name };
  }
  if (!out.error && /No session/.test(out.stderr ?? '')) return null;
  const why = out.error?.message ?? (out.stderr?.trim() || `exit status ${out.status}`);
  throw new Error(`loginctl show-session ${id} failed: ${why}`);
}

// A process the actions runner itself runs: its binary or working directory
// is under runnerDir. Unlike argv, a process can't fake its exe, and the cwd
// covers a runner started by hand as `./run.sh`.
function isRunnerProcess(proc, runnerDir) {
  const under = (path) => path === runnerDir || path?.startsWith(`${runnerDir}/`);
  return under(proc.exe) || under(proc.cwd);
}

// The processes in `procs` whose session has closed, grouped by session.
// `sessionInfo(id)` is loginctlSession or a stand-in. `exclude` holds pids
// to skip (this script's ancestry), and processes of `runnerUid` started
// from `runnerDir` are the runner's own.
export function findLeftovers(
  procs,
  { sessionInfo, exclude = new Set(), runnerUid, runnerDir = RUNNER_DIR },
) {
  const bySession = new Map();
  for (const proc of procs) {
    if (exclude.has(proc.pid)) continue;
    if (proc.uid === runnerUid && isRunnerProcess(proc, runnerDir)) continue;
    if (!bySession.has(proc.session)) bySession.set(proc.session, []);
    bySession.get(proc.session).push(proc);
  }
  const leftovers = [];
  for (const [session, sessionProcs] of bySession) {
    const info = sessionInfo(session);
    if (info !== null && info.state !== 'closing') continue;
    leftovers.push({
      session,
      user: info?.user ?? null,
      state: info?.state ?? 'unknown to logind',
      procs: sessionProcs,
    });
  }
  return leftovers;
}

// findLeftovers, re-scanned up to `attempts` times `delayMs` apart while
// any are found. logind leaves a session "closing" while it stops the
// scope (up to its 90 s stop timeout), so a job dispatched just after an
// operator logs out shouldn't fail on work that is already being killed.
export function settledLeftovers(scan, opts, { attempts = 4, delayMs = 5_000, sleep }) {
  let leftovers = findLeftovers(scan(), opts);
  for (let i = 1; i < attempts && leftovers.length > 0; i++) {
    sleep(delayMs);
    leftovers = findLeftovers(scan(), opts);
  }
  return leftovers;
}

// The command that stops a leftover session's processes. It targets the
// session's scope rather than PIDs, which may be reused before an operator
// acts on the message, and also clears zombies.
export function stopCommand(leftover) {
  return leftover.state === 'closing'
    ? `sudo loginctl terminate-session ${leftover.session}`
    : `sudo systemctl kill --signal=KILL session-${leftover.session}.scope`;
}

function shortCmdline(cmdline) {
  if (!cmdline) return '[no command line]';
  return cmdline.length > MAX_CMDLINE ? `${cmdline.slice(0, MAX_CMDLINE)}…` : cmdline;
}

// One line naming each leftover process, for a GitHub Actions annotation.
export function formatLeftovers(leftovers) {
  const sessions = leftovers.map((l) => {
    const who = l.user ? `${l.user}, ${l.state}` : l.state;
    const procs = l.procs.map((p) => `PID ${p.pid}: ${shortCmdline(p.cmdline)}`).join('; ');
    return `session ${l.session} (${who}): ${procs}`;
  });
  const stops = leftovers.map(stopCommand).join('; ');
  return (
    `processes left by a closed login session: ${sessions.join(' | ')}. ` +
    `Stop them (${stops}); work meant to outlive SSH goes in ` +
    'systemd-run --unit=<name> (scripts/bench/README.md §2.1)'
  );
}

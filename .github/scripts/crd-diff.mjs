#!/usr/bin/env node
// Compares each CRD against its state on BASE_SHA with `crdify`, failing on a
// change that breaks someone already running these CRDs. Checks are configured
// in .github/crd-diff-config.yaml.
//
// Uses `git show` + `file://` rather than crdify's `git://` source, which
// resolves refs with go-git and fails outright in a linked git worktree.

import { spawnSync } from 'node:child_process';
import { mkdtempSync, readdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const CRD_DIR = process.env.CRD_DIR || 'runtime-operator/config/crd/bases';
const CONFIG = '.github/crd-diff-config.yaml';

const baseSha = process.env.BASE_SHA;
if (!baseSha) {
  console.error('error: BASE_SHA env var is required');
  process.exit(2);
}

function git(args) {
  return spawnSync('git', args, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
}

// Exits non-zero when the path did not exist at that commit: the "new CRD" signal.
function showAtBase(path) {
  const r = git(['show', `${baseSha}:${path}`]);
  return r.status === 0 ? r.stdout : null;
}

function crdListAtBase() {
  const r = git(['ls-tree', '--name-only', baseSha, `${CRD_DIR}/`]);
  if (r.status !== 0) return [];
  return r.stdout.split('\n').filter((l) => l.endsWith('.yaml'));
}

const headCrds = readdirSync(CRD_DIR)
  .filter((f) => f.endsWith('.yaml'))
  .map((f) => `${CRD_DIR}/${f}`)
  .sort();

if (headCrds.length === 0) {
  console.error(`error: no CRDs found under ${CRD_DIR}`);
  process.exit(2);
}

const tmp = mkdtempSync(join(tmpdir(), 'crd-diff-'));
const failures = [];
let compared = 0;
let added = 0;

for (const path of headCrds) {
  const old = showAtBase(path);
  if (old === null) {
    console.log(`+ ${path}: new on this branch, nothing to compare`);
    added += 1;
    continue;
  }

  const oldPath = join(tmp, path.replaceAll('/', '_'));
  writeFileSync(oldPath, old);

  const r = spawnSync('crdify', ['--config', CONFIG, `file://${oldPath}`, `file://${path}`], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  if (r.error && r.error.code === 'ENOENT') {
    console.error('error: crdify not found on PATH; the workflow installs it before this step');
    process.exit(2);
  }

  compared += 1;
  const report = `${r.stdout || ''}${r.stderr || ''}`.trim();
  if (r.status === 0) {
    console.log(`  ${path}: compatible`);
    if (report) console.log(report.replace(/^/gm, '      '));
  } else {
    console.log(`! ${path}: INCOMPATIBLE`);
    console.log(report.replace(/^/gm, '      '));
    failures.push(path);
  }
}

// crdify cannot see a removal -- it compares a pair of files and there is no
// new file to pair -- but dropping a served CRD orphans every resource of it.
const headSet = new Set(headCrds);
for (const path of crdListAtBase()) {
  if (!headSet.has(path)) {
    console.log(`! ${path}: REMOVED -- orphans every existing resource of this kind`);
    failures.push(path);
  }
}

console.log(
  `\n${compared} CRD(s) compared against ${baseSha.slice(0, 12)}, ` +
    `${added} new, ${failures.length} incompatible.`,
);

if (failures.length > 0) {
  console.error(
    '\nIncompatible CRD changes:\n' +
      failures.map((f) => `  - ${f}`).join('\n') +
      '\n\nIf these changes are intentional and their upgrade impact is accounted ' +
      '\nfor, a maintainer can apply the `crd-diff-override` label to this PR and ' +
      '\nre-run the job.',
  );
  process.exit(1);
}

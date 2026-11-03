#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { appendFileSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const thresholds = { statements: 90, branches: 80 };

export function statementCounts(profile) {
  const lines = profile.trim().split('\n');
  if (lines.shift() !== 'mode: atomic') throw new Error('Expected atomic statement coverage');
  const blocks = new Map();
  if (!lines.length) throw new Error('Empty statement coverage profile');
  for (const line of lines) {
    const match = line.match(/^(.+:\d+\.\d+,\d+\.\d+) (\d+) (\d+)$/);
    if (!match) throw new Error(`Invalid coverage block: ${line}`);
    const statements = Number(match[2]);
    const executions = Number(match[3]);
    if (!Number.isSafeInteger(statements) || !Number.isSafeInteger(executions)) {
      throw new Error(`Invalid coverage counters: ${line}`);
    }
    const previous = blocks.get(match[1]);
    if (previous && previous.statements !== statements) {
      throw new Error(`Inconsistent statement count: ${line}`);
    }
    blocks.set(match[1], { statements, executed: executions > 0 || previous?.executed });
  }
  let covered = 0;
  let total = 0;
  for (const block of blocks.values()) {
    total += block.statements;
    if (block.executed) covered += block.statements;
  }
  return { covered, total };
}

export function branchCounts(conditions) {
  if (!Array.isArray(conditions)) throw new Error('Expected gobco condition array');
  let covered = 0;
  for (const condition of conditions) {
    if (typeof condition.Start !== 'string' || typeof condition.Code !== 'string') {
      throw new Error('Invalid gobco condition');
    }
    for (const key of ['TrueCount', 'FalseCount']) {
      if (!Number.isSafeInteger(condition[key]) || condition[key] < 0) {
        throw new Error(`Invalid gobco ${key}`);
      }
      if (condition[key] > 0) covered++;
    }
  }
  return { covered, total: conditions.length * 2 };
}

export function meetsThreshold({ covered, total }, threshold) {
  return total > 0 && covered * 100 >= total * threshold;
}

function percentage({ covered, total }) {
  return total ? `${(covered * 100 / total).toFixed(2)}%` : 'N/A';
}

async function run(module) {
  if (!['runtime-operator', 'runtime-gateway'].includes(module)) {
    throw new Error('Usage: node .github/scripts/go-coverage.mjs runtime-operator|runtime-gateway');
  }
  const root = fileURLToPath(new URL('../../', import.meta.url));
  const cwd = resolve(root, module);
  const out = resolve(root, 'target/go-coverage', module);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  if (module === 'runtime-operator' && !process.env.KUBEBUILDER_ASSETS) {
    throw new Error('KUBEBUILDER_ASSETS is required for operator integration tests');
  }
  const env = { ...process.env, GOWORK: 'off' };
  const commands = [];
  const failures = [];

  async function command(program, args, log) {
    commands.push({ program, args });
    const path = resolve(out, log);
    writeFileSync(path, '');
    return await new Promise((accept, reject) => {
      let stdout = '';
      const child = spawn(program, args, { cwd, env });
      child.stdout.on('data', (data) => {
        stdout += data;
        appendFileSync(path, data);
        process.stdout.write(data);
      });
      child.stderr.on('data', (data) => {
        appendFileSync(path, data);
        process.stderr.write(data);
      });
      child.on('error', reject);
      child.on('close', (code, signal) => {
        if (code !== 0) failures.push(`${program} ${args.join(' ')}: ${signal ?? code}`);
        accept({ stdout, code });
      });
    });
  }

  const go = await command('go', ['version'], 'go-version.txt');
  const gobco = process.env.GOBCO || 'gobco';
  const tool = await command(gobco, ['-version'], 'gobco-version.txt');
  const commit = await command('git', ['rev-parse', 'HEAD'], 'commit.txt');
  const status = await command('git', ['status', '--short'], 'working-tree.txt');
  const listed = await command('go', [
    'list', '-f', '{{.ImportPath}}\t{{.Dir}}\t{{if or .GoFiles .CgoFiles}}true{{else}}false{{end}}', './...',
  ], 'packages.txt');
  if (failures.length) throw new Error(failures.join('\n'));
  const packages = listed.stdout.trim().split('\n').map((line) => {
    const fields = line.split('\t');
    const [name, dir, production] = fields;
    if (fields.length !== 3 || !name || !dir || !['true', 'false'].includes(production)) {
      throw new Error(`Invalid package: ${line}`);
    }
    return { name, dir, production: production === 'true' };
  }).filter(({ name }) => !/\/test\/e2e(?:\/|$)/.test(name));
  if (!packages.length) throw new Error('No packages found');
  const names = packages.map(({ name }) => name);
  const provenance = {
    module, commit: commit.stdout.trim(), workingTree: status.stdout.trim(),
    collectedAt: new Date().toISOString(), go: go.stdout.trim(), gobco: tool.stdout.trim(),
    platform: `${process.platform}/${process.arch}`, node: process.version, thresholds, packages: names,
    scope: 'All module packages except test/e2e; generated code and test helpers included',
    branchScope: 'Per-package tests; gobco does not measure select statements or range loop outcomes',
    kubebuilderAssets: env.KUBEBUILDER_ASSETS ?? null,
    run: env.GITHUB_RUN_ID ? `${env.GITHUB_SERVER_URL}/${env.GITHUB_REPOSITORY}/actions/runs/${env.GITHUB_RUN_ID}` : null,
  };
  writeFileSync(resolve(out, 'provenance.json'), JSON.stringify(provenance, null, 2) + '\n');
  const tests = await command('go', [
    'test', '-count=1', '-timeout=10m', '-v', '-covermode=atomic',
    `-coverpkg=${names.join(',')}`, `-coverprofile=${out}/statements.out`, ...names,
  ], 'tests.txt');
  let statements = null;
  try {
    statements = statementCounts(readFileSync(resolve(out, 'statements.out'), 'utf8'));
    await command('go', ['tool', 'cover', '-func', `${out}/statements.out`], 'statements.txt');
    await command('go', ['tool', 'cover', '-html', `${out}/statements.out`, '-o', `${out}/statements.html`], 'html.txt');
  } catch (error) {
    failures.push(error.message);
  }

  const branches = { covered: 0, total: 0 };
  const branchPackages = [];
  const pending = packages.entries();
  await Promise.all(Array.from({ length: Math.min(3, packages.length) }, async () => {
    for (const [index, pkg] of pending) {
      const stats = `${out}/branches-${index}.json`;
      if (!pkg.production) {
        writeFileSync(stats, '[]\n');
        writeFileSync(`${out}/branches-${index}.txt`, 'No non-test Go files; tests run during statement collection.\n');
        branchPackages.push({ package: pkg.name, covered: 0, total: 0, passed: null, skipped: 'No non-test Go files' });
        continue;
      }
      const result = await command(gobco, [
        '-branch', '-stats', stats, '-test=-timeout=10m', pkg.dir,
      ], `branches-${index}.txt`);
      try {
        const counts = branchCounts(JSON.parse(readFileSync(stats, 'utf8')));
        branches.covered += counts.covered;
        branches.total += counts.total;
        branchPackages.push({ package: pkg.name, ...counts, passed: result.code === 0 });
      } catch (error) {
        failures.push(`${pkg.name}: ${error.message}`);
      }
    }
  }));
  branchPackages.sort((a, b) => a.package.localeCompare(b.package));
  const complete = failures.length === 0;
  const passed = complete && statements && meetsThreshold(statements, thresholds.statements)
    && meetsThreshold(branches, thresholds.branches);
  const report = {
    ...provenance, statements, branches, branchPackages, commands, failures,
    testsPassed: tests.code === 0, complete, passed: Boolean(passed),
  };
  writeFileSync(resolve(out, 'coverage.json'), JSON.stringify(report, null, 2) + '\n');
  const summary = [
    `## ${module} Go coverage`, '', `Commit: ${provenance.commit}`,
    `Statement tests: ${tests.code === 0 ? 'passed' : 'failed'}; collection: ${complete ? 'complete' : 'incomplete'}`,
    `Gold thresholds: ${passed ? 'passed' : 'failed'}`, '',
    '| Metric | Covered / total | Coverage | Gold target |',
    '| --- | ---: | ---: | ---: |',
    `| Statements | ${statements ? `${statements.covered} / ${statements.total}` : 'unavailable'} | ${statements ? percentage(statements) : 'N/A'} | ${thresholds.statements}% |`,
    `| Branch outcomes (gobco) | ${branches.covered} / ${branches.total} | ${percentage(branches)} | ${thresholds.branches}% |`, '',
    provenance.scope + '.', provenance.branchScope + '.',
    'Partial reports are diagnostic only. See CONTRIBUTING.md for badge evidence limits.', '',
    `Download ${module}-coverage for profiles, HTML, raw branch counts, test logs, and provenance.`, '',
  ].join('\n');
  writeFileSync(resolve(out, 'summary.md'), summary);
  if (env.GITHUB_STEP_SUMMARY) appendFileSync(env.GITHUB_STEP_SUMMARY, summary);
  console.log(summary);
  for (const failure of failures) console.error(failure);
  if (!passed) process.exitCode = 1;
}

if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  run(process.argv[2]).catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}

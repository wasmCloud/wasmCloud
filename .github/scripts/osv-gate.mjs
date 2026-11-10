#!/usr/bin/env node
// Decides whether an `osv-scanner --format json` report fails gate-audit.
//
// RustSec informational advisories (unmaintained, unsound, notice) are
// warnings, as they are for cargo-audit. Every other finding fails the gate,
// and so does any OSV malicious-package (`MAL-`) entry, whatever its metadata
// says.

import { readFile } from 'node:fs/promises';
import { relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// The RustSec informational kind of a vulnerability, or null if it is a
// vulnerability proper. OSV records it per affected package.
export function informational(vuln) {
  if (vuln.id?.startsWith('MAL-')) return null;
  for (const affected of vuln.affected ?? []) {
    const kind = affected.database_specific?.informational;
    if (kind) return kind;
  }
  return null;
}

// Splits a report's findings into blocking ones and warnings, one entry per
// lockfile, package version and advisory.
export function classify(report, root = process.cwd()) {
  const blocking = [];
  const warnings = [];
  const seen = new Set();
  for (const result of report.results ?? []) {
    const source = relative(root, result.source?.path ?? '') || '.';
    for (const { package: pkg, vulnerabilities } of result.packages ?? []) {
      for (const vuln of vulnerabilities ?? []) {
        const key = `${source} ${pkg?.name}@${pkg?.version} ${vuln.id}`;
        if (seen.has(key)) continue;
        seen.add(key);
        const kind = informational(vuln);
        const finding = { source, name: pkg?.name, version: pkg?.version, id: vuln.id, summary: vuln.summary ?? '', kind };
        (kind ? warnings : blocking).push(finding);
      }
    }
  }
  return { blocking, warnings };
}

function describe({ source, name, version, id, summary }) {
  return `${id} in ${name} ${version} (${source})${summary ? `: ${summary}` : ''}`;
}

async function main() {
  const [path] = process.argv.slice(2);
  if (!path) throw new Error('usage: osv-gate.mjs <osv-scanner-report.json>');
  const { blocking, warnings } = classify(JSON.parse(await readFile(path, 'utf8')));
  for (const finding of warnings) {
    console.log(`::warning title=RustSec ${finding.kind} advisory::${describe(finding)}`);
  }
  for (const finding of blocking) {
    console.log(`::error title=OSV finding::${describe(finding)}`);
  }
  if (blocking.length) {
    throw new Error(
      `osv-scanner found ${blocking.length} blocking finding${blocking.length === 1 ? '' : 's'}. Fix them, or ignore them in ` +
      'osv-scanner.toml with a VEX assessment as SECURITY.md describes.',
    );
  }
  console.log(`osv-scanner: no blocking findings (${warnings.length} informational warnings).`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    await main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

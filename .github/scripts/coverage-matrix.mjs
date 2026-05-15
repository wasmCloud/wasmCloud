#!/usr/bin/env node
// Asserts that a workflow's ITEMS list matches what's on disk under
// DISCOVERY_ROOT. Catches the failure modes that silently bypass CI:
//   - a new example/template directory was added (with .wash/config.yaml)
//     but never enrolled in ITEMS;
//   - an ITEMS entry's workdir was renamed or removed;
//   - REQUIRE_FILES (e.g. Cargo.toml) is missing from an otherwise-valid
//     directory — this workflow is scoped to a specific language;
//   - REQUIRE_README is set and a top-level subdirectory is missing
//     its README.md.
//
// Inputs (env):
//   ITEMS             JSON array shaped like plan-matrix.mjs's ITEMS.
//                     Each entry MUST have a `workdir:` field that points
//                     at the repo-relative directory containing
//                     .wash/config.yaml.
//   DISCOVERY_ROOT    Directory to scan, e.g. 'examples' or 'templates'.
//   REQUIRE_FILES     Optional JSON array of filenames that must coexist
//                     with .wash/config.yaml at each match — e.g.
//                     '["Cargo.toml"]' restricts coverage to Rust
//                     projects. Default: '[]'.
//   REQUIRE_README    Optional 'true': when set, every top-level
//                     subdirectory of DISCOVERY_ROOT that contains an
//                     enrolled item must have README.md at its root.

import { existsSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

function requireEnv(name) {
  const v = process.env[name];
  if (v === undefined || v === '') {
    console.error(`${name} env var is required`);
    process.exit(1);
  }
  return v;
}

function walk(dir, out = []) {
  for (const ent of readdirSync(dir, { withFileTypes: true })) {
    if (!ent.isDirectory()) continue;
    // target/ and .git/.wash/etc. never contain enrollable content and
    // can balloon scan time, so prune them.
    if (ent.name === 'target' || ent.name === 'node_modules' || ent.name.startsWith('.')) continue;
    const path = join(dir, ent.name);
    out.push(path);
    walk(path, out);
  }
  return out;
}

async function main() {
  const items = JSON.parse(requireEnv('ITEMS'));
  const root = requireEnv('DISCOVERY_ROOT');
  const requireFiles = JSON.parse(process.env.REQUIRE_FILES || '[]');
  const requireReadme = process.env.REQUIRE_README === 'true';

  const onDisk = walk(root).filter((d) => {
    if (!existsSync(join(d, '.wash', 'config.yaml'))) return false;
    return requireFiles.every((f) => existsSync(join(d, f)));
  });

  const onDiskSet = new Set(onDisk);
  const itemDirs = new Set(items.map((it) => it.workdir));

  const missing = [...onDiskSet].filter((d) => !itemDirs.has(d)).sort();
  const stale = items
    .filter((it) => !onDiskSet.has(it.workdir))
    .map((it) => `${it.workdir} (key=${it.key})`)
    .sort();

  const errors = [];
  if (missing.length) {
    errors.push(`dirs under ${root}/ with .wash/config.yaml not enrolled in ITEMS:`);
    missing.forEach((m) => errors.push(`  ${m}`));
  }
  if (stale.length) {
    errors.push('ITEMS entries whose workdir is missing on disk (or lacks required sibling files):');
    stale.forEach((s) => errors.push(`  ${s}`));
  }

  if (requireReadme) {
    // Only check top-level dirs that actually contain an enrolled workdir.
    // Avoids false positives if DISCOVERY_ROOT also holds unrelated subdirs.
    const enrolledTopDirs = new Set(
      [...onDiskSet].map((d) => d.split('/').slice(0, 2).join('/')),
    );
    const missingReadme = [...enrolledTopDirs]
      .filter((d) => !existsSync(join(d, 'README.md')))
      .sort();
    if (missingReadme.length) {
      errors.push('top-level dirs missing README.md:');
      missingReadme.forEach((d) => errors.push(`  ${d}/README.md`));
    }
  }

  if (errors.length) {
    errors.forEach((e) => console.error(e));
    process.exit(1);
  }

  console.error(`coverage OK: ${onDisk.length} dirs under ${root}/ enrolled (${items.length} items)`);
}

await main();

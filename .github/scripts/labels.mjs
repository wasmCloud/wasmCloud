#!/usr/bin/env node
// Projects .github/labels.yml into the two views its consumers need, so the
// label set and the path rules cannot drift apart: actions/labeler silently
// no-ops on a label that does not exist.
//
// Renders emit JSON on stdout; callers pipe through `yq -p=json -P` for YAML.
// YAML parsing is delegated to yq, as in runtime-operator-rbac-parity.mjs.

import { spawnSync } from 'node:child_process';

const LABELS = '.github/labels.yml';
const MAX_DESCRIPTION = 100;
const KNOWN_KEYS = ['color', 'description', 'aliases', 'rules'];

function loadLabels() {
  const r = spawnSync('yq', ['-o', 'json', '-I', '0', LABELS], {
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });
  if (r.error && r.error.code === 'ENOENT') {
    console.error('error: yq is not installed; it ships with GitHub-hosted runners');
    process.exit(1);
  }
  if (r.status !== 0) {
    console.error(`error: ${LABELS} is not valid YAML`);
    console.error(r.stderr.trim());
    process.exit(1);
  }
  try {
    return JSON.parse(r.stdout);
  } catch {
    console.error(`error: ${LABELS} did not round-trip to JSON`);
    process.exit(1);
  }
}

function validate(doc) {
  const errors = [];
  const fail = (m) => errors.push(m);

  if (doc === null || typeof doc !== 'object' || Array.isArray(doc)) {
    console.error(`error: ${LABELS} must be a mapping of label name -> { color, ... }`);
    process.exit(1);
  }

  const names = Object.keys(doc);
  const aliases = new Map();
  let ruled = 0;

  for (const name of names) {
    const v = doc[name];
    const where = `${LABELS} (${name})`;

    if (name.trim() === '') {
      fail(`${LABELS}: a label name is empty`);
      continue;
    }
    if (v === null || typeof v !== 'object' || Array.isArray(v)) {
      fail(`${where}: value must be a mapping`);
      continue;
    }

    if (typeof v.color !== 'string' || !/^#?[0-9a-fA-F]{6}$/.test(v.color)) {
      fail(`${where}: 'color' must be 6-digit hex, got ${JSON.stringify(v.color)}`);
    }

    if (v.description !== undefined) {
      if (typeof v.description !== 'string') {
        fail(`${where}: 'description' must be a string`);
      } else if (v.description.length > MAX_DESCRIPTION) {
        fail(
          `${where}: description is ${v.description.length} chars, ` +
            `GitHub's limit is ${MAX_DESCRIPTION}`,
        );
      }
    }

    if (v.aliases !== undefined) {
      if (!Array.isArray(v.aliases)) {
        fail(`${where}: 'aliases' must be a list`);
      } else {
        for (const a of v.aliases) {
          if (typeof a !== 'string' || a.trim() === '') {
            fail(`${where}: alias entries must be non-empty strings`);
            continue;
          }
          if (aliases.has(a)) {
            fail(`${where}: alias '${a}' is already claimed by '${aliases.get(a)}'`);
          }
          aliases.set(a, name);
        }
      }
    }

    if (v.rules !== undefined) {
      if (!Array.isArray(v.rules) || v.rules.length === 0) {
        fail(`${where}: 'rules' must be a non-empty list`);
      } else {
        ruled += 1;
      }
    }

    for (const key of Object.keys(v)) {
      if (!KNOWN_KEYS.includes(key)) {
        fail(`${where}: unknown key '${key}'`);
      }
    }
  }

  // A live label that is also an alias would be both kept and renamed.
  for (const [alias, owner] of aliases) {
    if (Object.hasOwn(doc, alias)) {
      fail(
        `${LABELS}: '${alias}' is declared as a label and also listed as an alias ` +
          `of '${owner}'; drop one or the sync result depends on ordering`,
      );
    }
  }

  if (errors.length > 0) {
    for (const e of errors) console.error(`error: ${e}`);
    console.error(`\n${errors.length} problem(s) found.`);
    process.exit(1);
  }

  console.error(
    `${LABELS}: ${names.length} labels, ${ruled} auto-applied by path, ` +
      `${aliases.size} alias(es). OK.`,
  );
}

function renderLabeler(doc) {
  const out = {};
  for (const [name, v] of Object.entries(doc)) {
    if (Array.isArray(v?.rules) && v.rules.length > 0) out[name] = v.rules;
  }
  if (Object.keys(out).length === 0) {
    console.error(`error: ${LABELS} defines no labels with 'rules'`);
    process.exit(1);
  }
  process.stdout.write(JSON.stringify(out, null, 2) + '\n');
}

function renderSync(doc) {
  const out = Object.entries(doc).map(([name, v]) => {
    const entry = { name, color: v.color };
    if (v.description) entry.description = v.description;
    if (v.aliases?.length) entry.aliases = v.aliases;
    return entry;
  });
  process.stdout.write(JSON.stringify(out, null, 2) + '\n');
}

const doc = loadLabels();
const cmd = process.argv[2];
switch (cmd) {
  case 'validate':
    validate(doc);
    break;
  case 'render-labeler':
    validate(doc);
    renderLabeler(doc);
    break;
  case 'render-sync':
    validate(doc);
    renderSync(doc);
    break;
  default:
    console.error('usage: labels.mjs <validate|render-labeler|render-sync>');
    process.exit(2);
}

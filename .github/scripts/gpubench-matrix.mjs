#!/usr/bin/env node
// Resolve a gpubench.yml dispatch into the boxes to run on, the ref to label
// rows with, the ref to check out, and whether the results may be published.
//
// Reads INPUT_REF, INPUT_BOX (one or more box labels, comma or space
// separated; see gpubench-boxes.mjs), INPUT_SMOKE and INPUT_PUBLISH_PR
// ("true" or "false") and DEFAULT_REF (github.ref); writes boxes, ref,
// checkout_ref and publish to $GITHUB_OUTPUT. Ref spellings match bench.yml's:
// a branch, tag or sha, or a pull request as #N, pull/N or pr/N.

import { appendFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

import { parseBox } from './gpubench-boxes.mjs';

export function resolve({ ref, defaultRef, box, smoke, publishPr = false }) {
  const boxes = [...new Set(box.split(/[\s,]+/).filter(Boolean))];
  if (boxes.length === 0) throw new Error('no box to run on');
  boxes.forEach(parseBox);

  let label = ref || defaultRef;
  if (!label) throw new Error('no ref to bench');
  label = label.replace(/^refs\/heads\//, '').replace(/^refs\/tags\//, '');
  let checkoutRef = label;
  const pull = label.match(/^(?:#|pull\/|pr\/)(.*)$/);
  if (pull) {
    if (!/^[0-9]+$/.test(pull[1])) {
      throw new Error(`invalid pull-request ref '${label}' (expected #N, pull/N, or pr/N)`);
    }
    label = `pull/${pull[1]}`;
    checkoutRef = `refs/pull/${pull[1]}/head`;
  }

  // Smoke numbers prove the pipeline works and aren't worth keeping.
  // Pull-request rows describe code that may never land, so they publish only
  // when asked; they keep the pull/N ref, which the GPU view filters on.
  const isPull = label.startsWith('pull/');
  if (publishPr && !isPull) throw new Error(`publish_pr needs a pull-request ref, not '${label}'`);
  const publish = smoke !== true && (!isPull || publishPr === true);
  return { boxes, ref: label, checkoutRef, publish };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const out = resolve({
    ref: process.env.INPUT_REF ?? '',
    defaultRef: process.env.DEFAULT_REF ?? '',
    box: process.env.INPUT_BOX ?? '',
    smoke: process.env.INPUT_SMOKE === 'true',
    publishPr: process.env.INPUT_PUBLISH_PR === 'true',
  });
  const lines = [
    `boxes=${JSON.stringify(out.boxes)}`,
    `ref=${out.ref}`,
    `checkout_ref=${out.checkoutRef}`,
    `publish=${out.publish}`,
  ];
  for (const line of lines) {
    if (/[\r\n]/.test(line)) throw new Error('refusing a multi-line output');
    console.log(line);
  }
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${lines.join('\n')}\n`);
}

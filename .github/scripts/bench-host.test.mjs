import assert from 'node:assert/strict';
import test from 'node:test';

import { DEFAULT_HOST_LABEL, hostnameVar, labelRows, parseHostLabel, rowHostLabel } from './bench-host.mjs';

test('accepts runner-label-shaped host labels only', () => {
  for (const label of ['hetzner', 'aws-c7i', 'x86-64-metal', 'a1']) {
    assert.equal(parseHostLabel(label), label);
  }
  for (const label of ['', 'Hetzner', 'aws_c7i', '-aws', 'aws-', 'aws--c7i', 'aws c7i', 'a\nb', undefined]) {
    assert.throws(() => parseHostLabel(label), /invalid bench host label/);
  }
});

test('maps a label to its hostname variable', () => {
  assert.equal(hostnameVar(DEFAULT_HOST_LABEL), 'WASMCLOUD_BENCH_HOSTNAME');
  assert.equal(hostnameVar('aws-c7i-metal'), 'WASMCLOUD_BENCH_AWS_C7I_METAL_HOSTNAME');
  assert.throws(() => hostnameVar('AWS'), /invalid bench host label/);
});

test('treats unlabeled rows as the dedicated host', () => {
  assert.equal(rowHostLabel({}), 'hetzner');
  assert.equal(rowHostLabel({ host_label: 'aws-c7i' }), 'aws-c7i');
});

test('labels unlabeled rows and leaves labeled lines untouched', () => {
  const labeled = '{"metric":"mean_ns","host_label":"hetzner","value":1.0}';
  const out = labelRows(`{"metric":"mean_ns"}\n${labeled}\n`, 'aws-c7i');
  assert.equal(out, `{"metric":"mean_ns","host_label":"aws-c7i"}\n${labeled}\n`);
  assert.equal(labelRows('', 'aws-c7i'), '');
});

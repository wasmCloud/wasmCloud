import assert from 'node:assert/strict';
import test from 'node:test';

import { parseBox } from './gpubench-boxes.mjs';
import { resolve } from './gpubench-matrix.mjs';

test('several boxes, the default ref, published', () => {
  assert.deepEqual(resolve({ ref: '', defaultRef: 'refs/heads/main', box: 'metal-m4pro, cuda-l4', smoke: false }), {
    boxes: ['metal-m4pro', 'cuda-l4'],
    ref: 'main',
    checkoutRef: 'main',
    publish: true,
  });
});

test('a tag on one box', () => {
  const out = resolve({ ref: 'refs/tags/v2.12.0', defaultRef: 'refs/heads/main', box: 'cuda-l4', smoke: false });
  assert.deepEqual(out.boxes, ['cuda-l4']);
  assert.equal(out.ref, 'v2.12.0');
  assert.equal(out.publish, true);
});

test('a box named twice runs once', () => {
  assert.deepEqual(resolve({ ref: 'main', defaultRef: '', box: 'cuda-l4 cuda-l4,', smoke: false }).boxes, ['cuda-l4']);
});

test('pull-request spellings check out the head ref and never publish', () => {
  for (const ref of ['#123', 'pull/123', 'pr/123']) {
    const out = resolve({ ref, defaultRef: 'refs/heads/main', box: 'metal-m4pro', smoke: false });
    assert.equal(out.ref, 'pull/123');
    assert.equal(out.checkoutRef, 'refs/pull/123/head');
    assert.equal(out.publish, false);
  }
  assert.throws(() => resolve({ ref: '#12a', defaultRef: '', box: 'metal-m4pro', smoke: false }));
});

test('publish_pr publishes a pull-request ref under its pull/N label', () => {
  const out = resolve({ ref: '#5898', defaultRef: 'refs/heads/main', box: 'cuda-l4', smoke: false, publishPr: true });
  assert.equal(out.ref, 'pull/5898');
  assert.equal(out.checkoutRef, 'refs/pull/5898/head');
  assert.equal(out.publish, true);
  assert.equal(resolve({ ref: '#5898', defaultRef: '', box: 'cuda-l4', smoke: true, publishPr: true }).publish, false);
});

test('publish_pr is refused for a ref that is not a pull request', () => {
  for (const ref of ['', 'main', 'v2.12.0']) {
    assert.throws(() => resolve({ ref, defaultRef: 'refs/heads/main', box: 'cuda-l4', smoke: false, publishPr: true }), /publish_pr/);
  }
});

test('smoke runs never publish', () => {
  assert.equal(resolve({ ref: 'main', defaultRef: '', box: 'cuda-l4', smoke: true }).publish, false);
});

test('unknown boxes are refused', () => {
  for (const box of ['', ' , ', 'both', 'cuda', 'metal', 'vulkan-sw', 'CUDA-L4', 'cuda-', 'cuda_l4', 'cuda-l4,rocm-mi300']) {
    assert.throws(() => resolve({ ref: 'main', defaultRef: '', box, smoke: false }), undefined, box);
  }
});

test('a label names its kind and hostname variable', () => {
  assert.deepEqual(parseBox('cuda-a10g-24g'), {
    label: 'cuda-a10g-24g',
    kind: 'cuda',
    hostnameVar: 'WASMCLOUD_BENCH_CUDA_A10G_24G_HOSTNAME',
  });
  assert.equal(parseBox('metal-m4pro').kind, 'metal');
});

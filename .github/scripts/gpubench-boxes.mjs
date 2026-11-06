// gpubench box labels. A box is named by its runner label, `<kind>-<class>`:
// the kind picks the backend and the platform (cuda: Linux/x64 with an NVIDIA
// GPU, metal: macOS/arm64), and the class names the hardware, e.g. `cuda-l4`
// or `metal-m4pro`. The label is the box's bench host label (bench-host.mjs),
// so rows carry it as host_label and two hardware classes never share a
// history. Adding a box needs no change here: register a runner with the label
// and set the repo variable hostnameVar(label) names.

import { hostnameVar } from './bench-host.mjs';

export const KINDS = ['cuda', 'metal'];

// A bench host label with a kind word in front of at least one class word.
const LABEL = /^([a-z]+)-[a-z0-9]+(?:-[a-z0-9]+)*$/;

export function parseBox(label) {
  const m = LABEL.exec(label);
  if (!m || !KINDS.includes(m[1])) {
    throw new Error(`invalid box '${label}' (expected <kind>-<class> with kind ${KINDS.join(' or ')}, e.g. cuda-l4)`);
  }
  return { label, kind: m[1], hostnameVar: hostnameVar(label) };
}

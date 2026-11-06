// Bench host labels: the runner label a bench job targets, and the series its
// rows belong to. `hetzner` is the dedicated host; any other label is a
// short-lived host registered with install-runner.sh --label.

export const DEFAULT_HOST_LABEL = 'hetzner';

const LABEL_RE = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

export function parseHostLabel(label) {
  if (typeof label !== 'string' || !LABEL_RE.test(label)) {
    throw new Error(`invalid bench host label '${label}' (want lowercase words joined by '-')`);
  }
  return label;
}

// The repo variable holding the label's expected hostname.
export function hostnameVar(label) {
  parseHostLabel(label);
  if (label === DEFAULT_HOST_LABEL) return 'WASMCLOUD_BENCH_HOSTNAME';
  return `WASMCLOUD_BENCH_${label.toUpperCase().replaceAll('-', '_')}_HOSTNAME`;
}

// Rows from before rows carried a label are the dedicated host's.
export function rowHostLabel(row) {
  return row.host_label ?? DEFAULT_HOST_LABEL;
}

// Labels the unlabeled rows of a JSONL string, as an older benched ref's
// bench-tools writes them. Labeled lines pass through unchanged.
export function labelRows(jsonl, label) {
  const lines = jsonl.split('\n').filter((line) => line.length > 0).map((line) => {
    const row = JSON.parse(line);
    return 'host_label' in row ? line : JSON.stringify({ ...row, host_label: label });
  });
  return lines.length ? `${lines.join('\n')}\n` : '';
}

# Unreleased

The operator now uses Go's `log/slog`. Existing `--zap-*` flags remain aliases
for the new logging flags.

- Production logging no longer samples repeated messages. High-volume logs may
  increase after upgrading; use `--log-level` or collection-side controls to
  manage volume.
- Default text logs use slog's key=value format. JSON retains the existing field
  names and timestamp defaults. Controller verbosity is available in `v`.
- Kubernetes object pointers, passed directly or through `logr.Marshaler`, are
  logged as identity in every mode, including the default Helm deployment.
  `--log-full-objects` explicitly enables full contents, including Secret data.
  Objects passed by value or inside slices and maps are not redacted.
- A top-level `time` field holding a `time.Time` is renamed to `ts` and encoded
  like the record timestamp. Fields inside a group keep their names.
- Standard-library HTTP diagnostics remain visible at error thresholds without
  added application stack traces.

#!/usr/bin/env bash
# Run a command while holding the bench box's lock. Every bench job on a
# shared box takes it, wasmCloud's and Cosmonic Control's alike, so two
# benches never share the machine.
#
#   .github/scripts/bench-box-lock.sh <command> [args...]
#
# It lives beside bench-preflight.mjs, not under scripts/, so a workflow runs
# its own copy from the workflow-tools checkout: a benched ref older than the
# lock has no copy of its own, and a benched ref is not trusted to take it.

set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: $0 <command> [args...]" >&2
  exit 2
fi

LOCK="${BENCH_BOX_LOCK:-/var/lock/bench-box.lock}"

# /var/lock is a tmpfs, so the file may be gone after a reboot. Read-only,
# because the lock then works whichever runner user made the file.
[ -e "$LOCK" ] || (umask 0 && : >>"$LOCK")
exec 9<"$LOCK"
if ! flock -n 9; then
  echo "waiting for $LOCK" >&2
  start=$SECONDS
  flock 9
  echo "waited $((SECONDS - start))s for $LOCK" >&2
fi
echo "holding $LOCK" >&2

# Without the descriptor, so a process the command leaves running cannot keep
# the lock after this script exits.
"$@" 9<&-

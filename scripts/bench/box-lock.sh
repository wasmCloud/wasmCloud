#!/usr/bin/env bash
# Run a command while holding the bench box's lock. Every bench job on a
# shared box takes it, wasmCloud's and Cosmonic Control's alike, so two
# benches never share the machine.
#
#   scripts/bench/box-lock.sh <command> [args...]

set -euo pipefail

LOCK="${BENCH_BOX_LOCK:-/var/lock/bench-box.lock}"

# /var/lock is a tmpfs, so the file may be gone after a reboot. Read-only,
# because the lock then works whichever runner user made the file.
[ -e "$LOCK" ] || (umask 0 && : >>"$LOCK")
exec 9<"$LOCK"
echo "waiting for $LOCK" >&2
flock 9
echo "holding $LOCK" >&2

# Without the descriptor, so a process the command leaves running cannot keep
# the lock after this script exits.
"$@" 9<&-

#!/usr/bin/env bash
# Compare two scripts/k6bench result directories, baseline first:
#
#   compare.sh bench-results/<baseline> bench-results/<candidate> [--markdown]
#
# Both runs must be the same scenario at the same profile and rate. Laptop
# runs swing by 10–20 % between identical runs; compare bench-host runs, or
# repeat a local pair before believing a move.

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
[ $# -ge 2 ] || { sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }

baseline="$(cd "$1" && pwd)"
candidate="$(cd "$2" && pwd)"
shift 2
cd "$repo" && exec cargo run -p bench-tools --quiet -- k6 delta "$baseline" "$candidate" "$@"

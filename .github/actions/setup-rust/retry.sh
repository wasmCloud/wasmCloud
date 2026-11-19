#!/usr/bin/env bash
# Retry idempotent toolchain downloads, never build or test commands. Source
# this file so the caller's argument boundaries and final exit status survive.
#
# rustup exits 1 whatever went wrong, so only its message tells a request that
# cannot succeed (a misspelt target, a component the channel lacks) from a
# download that may. The first kind fails at once: retrying it costs time and
# annotates the job with warnings that blame the network.
retry_download() {
  local permanent='does not support target|does not contain component|is unavailable for download|invalid toolchain name'
  local attempt codes log status
  log="$(mktemp)"
  for attempt in 1 2 3; do
    # Copy stderr to the log while it still streams to the job output; stdout
    # passes through untouched. `|| true` keeps a caller's `set -e` from ending
    # the script before the failure is classified.
    { "$@" 2>&1 >&3 | tee "$log" >&2; codes=("${PIPESTATUS[@]}"); } 3>&1 || true
    status="${codes[0]}"
    if [ "$status" -eq 0 ]; then
      rm -f "$log"
      return 0
    fi
    if [ "$attempt" -eq 3 ] || grep -Eq "$permanent" "$log"; then
      rm -f "$log"
      return "$status"
    fi
    echo "::warning::Toolchain setup failed (attempt ${attempt}/3); retrying in $((attempt * 5)) seconds" >&2
    sleep "$((attempt * 5))"
  done
}

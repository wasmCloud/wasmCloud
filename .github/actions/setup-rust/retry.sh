#!/usr/bin/env bash
# Retry idempotent toolchain downloads, never build or test commands. Source
# this file so the caller's argument boundaries and final exit status survive.
retry_download() {
  local attempt status
  for attempt in 1 2 3; do
    if "$@"; then
      return 0
    else
      status=$?
    fi
    if [ "$attempt" -eq 3 ]; then
      return "$status"
    fi
    echo "::warning::Toolchain setup failed (attempt ${attempt}/3); retrying in $((attempt * 5)) seconds" >&2
    sleep "$((attempt * 5))"
  done
}

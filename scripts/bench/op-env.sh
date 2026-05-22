#!/usr/bin/env bash
# Fetch the wasmCloud bench-host values from 1Password and export them into
# the current shell. Source me, don't execute me:
#
#   source scripts/bench/op-env.sh
#
# After sourcing, these are exported:
#
#   WASMCLOUD_BENCH_HOST_IP    public IPv4 — consumed by the Ansible inventory and
#                    every shell script in this directory
#   WASMCLOUD_BENCH_HOSTNAME   the box's hostname (matches bench-preflight.mjs's assertion)
#   WASMCLOUD_BENCH_HOST_IPV6  (optional) public IPv6, if stored on the item
#
# 1Password item name defaults to `WASMCLOUD_BENCH_BOX`. Override with:
#
#   WASMCLOUD_BENCH_OP_ITEM=<other-item-name> source scripts/bench/op-env.sh
#
# Vault is auto-detected (default vault). Pass `--vault <name>` via
# WASMCLOUD_BENCH_OP_ARGS if you need a specific one:
#
#   WASMCLOUD_BENCH_OP_ARGS="--vault Personal" source scripts/bench/op-env.sh
#
# Requirements:
#   - 1Password CLI v2+ (`brew install --cask 1password-cli`)
#   - Signed in: either `eval $(op signin)` or biometric integration enabled
#     in the desktop app under Settings → Developer
#
# The 1Password item must carry at least these fields (case-sensitive):
#   WASMCLOUD_BENCH_HOST_IP, WASMCLOUD_BENCH_HOSTNAME, WASMCLOUD_BENCH_HOST_IPV6 (optional)

# Detect that we're being sourced, not executed — we use `return` not `exit`
# so a failure doesn't kill the caller's shell. `return` only works inside
# a sourced script or a function.
if ! (return 0 2>/dev/null); then
  echo "op-env.sh: source me, don't execute me." >&2
  echo "  source scripts/bench/op-env.sh" >&2
  exit 1
fi

_op_env_item="${WASMCLOUD_BENCH_OP_ITEM:-WASMCLOUD_BENCH_BOX}"
# shellcheck disable=SC2206  # intentional word-split for op CLI args
_op_env_args=( ${WASMCLOUD_BENCH_OP_ARGS:-} )

if ! command -v op >/dev/null 2>&1; then
  echo "op-env.sh: 1Password CLI (op) not installed." >&2
  echo "  brew install --cask 1password-cli" >&2
  unset _op_env_item _op_env_args
  return 1
fi

# `op whoami` exits non-zero if there's no active session. The check is
# cheap and gives a clearer error than letting `op item get` blow up later.
if ! op whoami >/dev/null 2>&1; then
  echo "op-env.sh: not signed in to 1Password CLI." >&2
  echo "  - GUI integration: 1Password app → Settings → Developer →" >&2
  echo "                     'Integrate with 1Password CLI' (biometric)" >&2
  echo "  - Or shell signin: eval \$(op signin)" >&2
  unset _op_env_item _op_env_args
  return 1
fi

# --reveal forces op to print the field's actual value rather than the
# masked placeholder it shows in interactive output. Required for use as
# an env var.
_op_env_get() {
  op item get "$_op_env_item" "${_op_env_args[@]}" --field "$1" --reveal 2>/dev/null
}

_ip=$(_op_env_get WASMCLOUD_BENCH_HOST_IP) || true
_hostname=$(_op_env_get WASMCLOUD_BENCH_HOSTNAME) || true
_ipv6=$(_op_env_get WASMCLOUD_BENCH_HOST_IPV6) || true

if [ -z "$_ip" ] || [ -z "$_hostname" ]; then
  echo "op-env.sh: required fields missing from 1Password item '$_op_env_item'." >&2
  [ -z "$_ip" ]       && echo "  - WASMCLOUD_BENCH_HOST_IP    (missing or empty)" >&2
  [ -z "$_hostname" ] && echo "  - WASMCLOUD_BENCH_HOSTNAME   (missing or empty)" >&2
  echo >&2
  echo "Edit the item in 1Password to add them, or override the item name with:" >&2
  echo "  WASMCLOUD_BENCH_OP_ITEM=<name> source scripts/bench/op-env.sh" >&2
  unset _op_env_item _op_env_args _op_env_get _ip _hostname _ipv6
  return 1
fi

export WASMCLOUD_BENCH_HOST_IP="$_ip"
export WASMCLOUD_BENCH_HOSTNAME="$_hostname"
if [ -n "$_ipv6" ]; then
  export WASMCLOUD_BENCH_HOST_IPV6="$_ipv6"
fi

echo "op-env.sh: exported WASMCLOUD_BENCH_HOST_IP=${_ip}, WASMCLOUD_BENCH_HOSTNAME=${_hostname}${_ipv6:+, WASMCLOUD_BENCH_HOST_IPV6=${_ipv6}}" >&2
echo "           (source: 1Password item '$_op_env_item')" >&2

unset _op_env_item _op_env_args _op_env_get _ip _hostname _ipv6

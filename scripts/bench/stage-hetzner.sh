#!/usr/bin/env bash
# Stage the wasmCloud bench server: install Ubuntu 24.04 onto the raw NVMes
# via Hetzner installimage, then reboot.
#
# Prereqs:
#   - Server booted into Hetzner rescue and reachable as root
#   - ~/.ssh/hetzner_bench keypair exists and is authorized in rescue
#   - Host keys pinned in ~/.ssh/known_hosts
#
# This script is destructive: installimage WIPES both /dev/nvme0n1 and
# /dev/nvme1n1. There is a final confirmation prompt.
#
# After reboot:
#   - Hetzner regenerates SSH host keys, so the rescue fingerprints in
#     ~/.ssh/known_hosts will not match. Refresh them, then run the
#     Ansible playbook (scripts/bench/ansible/provision.yml).

set -euo pipefail

# WASMCLOUD_BENCH_HOST_IP and WASMCLOUD_BENCH_HOSTNAME are read from the operator's environment
# — they are kept out of the repo. Populate from 1Password via:
#   source scripts/bench/op-env.sh    (item: WASMCLOUD_BENCH_BOX)
: "${WASMCLOUD_BENCH_HOST_IP:?WASMCLOUD_BENCH_HOST_IP not set — source scripts/bench/op-env.sh first}"
: "${WASMCLOUD_BENCH_HOSTNAME:?WASMCLOUD_BENCH_HOSTNAME not set — source scripts/bench/op-env.sh first}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/hetzner_bench}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ssh_opts=(-i "$SSH_KEY" -o BatchMode=yes -o ConnectTimeout=15)
ssh_cmd() { ssh "${ssh_opts[@]}" "root@${WASMCLOUD_BENCH_HOST_IP}" "$@"; }
scp_cmd() { scp "${ssh_opts[@]}" "$@"; }

step() { printf '\n=== %s ===\n' "$*"; }

step "Sanity: server must be in Hetzner rescue"
hn=$(ssh_cmd 'hostname')
if [ "$hn" != "rescue" ]; then
  echo "hostname is '$hn', expected 'rescue'." >&2
  echo "Re-arm rescue mode in Hetzner Robot, reboot, then re-run." >&2
  exit 1
fi

step "Copy autosetup + post-install hook to rescue"
# Render hetzner-autosetup.cfg.tmpl with the operator-provided hostname;
# the .tmpl file is committed, the rendered config is generated on demand
# and lives only on the rescue system. This keeps the actual hostname out
# of the repo (defense-in-depth — see scripts/bench/README.md §1).
tmpcfg=$(mktemp)
trap 'rm -f "$tmpcfg"' EXIT
sed "s/__BENCH_HOSTNAME__/${WASMCLOUD_BENCH_HOSTNAME}/g" \
    "${SCRIPT_DIR}/hetzner-autosetup.cfg.tmpl" > "$tmpcfg"
scp_cmd "$tmpcfg" "root@${WASMCLOUD_BENCH_HOST_IP}:/autosetup"
scp_cmd "${SCRIPT_DIR}/hetzner-postinstall.sh" "root@${WASMCLOUD_BENCH_HOST_IP}:/tmp/postinstall.sh"
ssh_cmd 'chmod +x /tmp/postinstall.sh'

step "About to run installimage — this will WIPE both NVMes"
ssh_cmd 'lsblk -o NAME,SIZE,TYPE,FSTYPE | grep -E "^nvme"'
read -rp "Type WIPE to proceed: " ans
[ "$ans" = "WIPE" ] || { echo "Aborted."; exit 1; }

step "Running installimage"
# -a   auto / batch
# -c   config
# -x   post-install hook (runs in chroot)
ssh_cmd '/root/.oldroot/nfs/install/installimage -a -c /autosetup -x /tmp/postinstall.sh'

step "Reboot into installed OS"
# `reboot` will close the SSH connection; ignore the resulting error.
ssh_cmd 'reboot' || true

cat <<EOM

Box is rebooting. Once it's back (~2-3 minutes):

  1. Refresh host keys (installimage regenerated them):
       ssh-keygen -R ${WASMCLOUD_BENCH_HOST_IP}
       ssh-keyscan -t rsa,ecdsa,ed25519 ${WASMCLOUD_BENCH_HOST_IP} >> ~/.ssh/known_hosts

  2. Confirm key auth still works (your authorized_keys was carried over):
       ssh -i ${SSH_KEY} root@${WASMCLOUD_BENCH_HOST_IP} 'hostname; uname -srm'

  3. Run provisioning (from this checkout):
       cd ${SCRIPT_DIR}/ansible && ansible-playbook provision.yml
EOM

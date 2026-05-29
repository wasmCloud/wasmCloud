#!/usr/bin/env bash
# Runs inside the chroot of the freshly-installed Ubuntu before first boot.
# Bake bench-host kernel/cpufreq tweaks into the installed system so they
# survive reboots and don't have to be reapplied between bench runs.
#
# Tweaks (matches user choice during staging):
#   1. nosmt           - kernel disables hyperthreading, leaving 6 physical
#                        cores. Cleaner numbers, less variance.
#   2. isolcpus=5      - reserve CPU 5 from the general scheduler. Plus
#                        nohz_full=5 + rcu_nocbs=5 to also stop the
#                        scheduler tick and offload RCU callbacks off
#                        that core. Bench processes that want a quiet
#                        core taskset themselves onto CPU 5 (currently
#                        only the gungraun bench; criterion benches are
#                        multi-threaded and don't pin).
#   3. governor=perf   - systemd unit pins every CPU's scaling_governor to
#                        `performance` on boot, eliminating freq scaling
#                        jitter during runs.

set -euo pipefail

# CPU index to isolate. With nosmt active, the box has CPUs 0..5; reserving
# the last one leaves 5 cores for the runner + system, which is plenty.
# Keep in sync with WASMCLOUD_BENCH_ISOLATED_CPU in scripts/bench/run-bench.sh.
ISOLATED_CPU=5

# --- 1. nosmt + isolcpus via GRUB cmdline ------------------------------------
# Hetzner ships /etc/default/grub.d/hetzner.cfg which *resets*
# GRUB_CMDLINE_LINUX_DEFAULT="consoleblank=0" after /etc/default/grub is
# sourced. Editing /etc/default/grub directly is therefore clobbered. Use
# our own drop-in that sorts after the Hetzner one (zz-*) and appends.
mkdir -p /etc/default/grub.d
cat > /etc/default/grub.d/zz-bench.cfg <<GRUB
# wasmCloud bench host: append the bench-time kernel tweaks.
#   nosmt         - disable SMT/hyperthreading entirely.
#   isolcpus      - remove CPU from general scheduler load-balancing.
#   nohz_full     - stop the periodic scheduler tick on that CPU when
#                   only one runnable task is on it (kills timer-driven
#                   measurement jitter).
#   rcu_nocbs     - offload RCU callbacks off that CPU to a kthread.
GRUB_CMDLINE_LINUX_DEFAULT="\${GRUB_CMDLINE_LINUX_DEFAULT} nosmt isolcpus=${ISOLATED_CPU} nohz_full=${ISOLATED_CPU} rcu_nocbs=${ISOLATED_CPU}"
GRUB
update-grub

# --- 2. governor=performance via systemd unit --------------------------------
cat > /etc/systemd/system/cpu-performance.service <<'UNIT'
[Unit]
Description=Pin all CPUs to the performance scaling_governor (bench host)
After=multi-user.target
ConditionPathExists=/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c 'for g in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do echo performance > "$g"; done'

[Install]
WantedBy=multi-user.target
UNIT

systemctl enable cpu-performance.service

# --- 3. Marker so we can prove post-install ran on the box -------------------
{
  echo "wasmcloud-bench post-install"
  echo "stamp: $(date -u +%FT%TZ)"
  echo "kernel-tweak: nosmt isolcpus=${ISOLATED_CPU} nohz_full=${ISOLATED_CPU} rcu_nocbs=${ISOLATED_CPU}"
  echo "cpufreq-governor: performance"
  echo "isolated-cpu: ${ISOLATED_CPU}"
} > /etc/wasmcloud-bench-stage

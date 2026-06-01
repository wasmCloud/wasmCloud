# wasmCloud bench runner

Operator's runbook for the dedicated `wash-runtime` benchmark host
(`<WASMCLOUD_BENCH_HOSTNAME>`) and the GitHub-driven pipeline that runs against
it. Read top-to-bottom for a fresh setup; jump to a section for an
existing host.

> **TL;DR.** A single Hetzner box, locked-down kernel/cpufreq settings
> for reproducibility, a self-hosted GitHub Actions runner that fires
> on `workflow_dispatch`, S3 for artifacts, CloudFront in front for
> public reads, and [https://wasmcloud.github.io/arewefastyet](https://https://wasmcloud.github.io/arewefastyet)
> as the trend dashboard (separate repo:
> [`wasmCloud/arewefastyet`](https://github.com/wasmCloud/arewefastyet)).

## Contents

1. [Hardware](#1-hardware)
2. [SSH access](#2-ssh-access)
3. [OS install (Hetzner rescue → Ubuntu)](#3-os-install-hetzner-rescue--ubuntu)
4. [Bench-time kernel/cpufreq tweaks](#4-bench-time-kernelcpufreq-tweaks)
5. [Toolchain + dependencies](#5-toolchain--dependencies)
6. [Self-hosted GitHub Actions runner](#6-self-hosted-github-actions-runner)
7. [Pipeline architecture](#7-pipeline-architecture)
8. [AWS bootstrap (S3 + CloudFront + IAM)](#8-aws-bootstrap-s3--cloudfront--iam)
9. [Running a bench](#9-running-a-bench)
10. [Pre-flight assertions](#10-pre-flight-assertions)
11. [What gets stored where](#11-what-gets-stored-where)
12. [Re-staging from scratch](#12-re-staging-from-scratch)
13. [Troubleshooting](#13-troubleshooting)
14. [Files in this directory](#14-files-in-this-directory)
15. [Dependency cadence](#15-dependency-cadence)
16. [Out of scope](#16-out-of-scope)

---

## 1. Hardware

Single dedicated Hetzner host. Live detail on the box via `lscpu`, `lsblk`,
`free -h`, `uname -srm`; summary:

|                      |                                                                                    |
| -------------------- | ---------------------------------------------------------------------------------- |
| **Provider**         | Hetzner                                                                            |
| **CPU**              | AMD Ryzen 5 3600 (6 cores / 12 threads, Zen 2, AVX2 + AES-NI + SHA-NI; no AVX-512) |
| **Base / max clock** | ~3.6 GHz / 4.208 GHz                                                               |
| **RAM**              | 62 GiB DDR4, no swap                                                               |
| **Storage**          | 2× Samsung MZVL2512HCJQ NVMe, 476.94 GiB each (RAID1 via mdadm)                    |
| **NIC**              | 1× 1 GbE                                                                           |
| **Boot mode**        | Legacy BIOS                                                                        |
| **OS**               | Ubuntu 24.04 LTS (Noble) amd64                                                     |
| **Hostname**         | `<WASMCLOUD_BENCH_HOSTNAME>` (operator runbook — 1Password)                        |
| **IPv4**             | `<WASMCLOUD_BENCH_HOST_IP>` (operator runbook — 1Password)                         |
| **IPv6**             | `<WASMCLOUD_BENCH_HOST_IPV6>` (operator runbook — 1Password)                       |

The box is dedicated to benchmarking — nothing else runs on it. Don't
co-locate workloads if numbers are to mean anything across runs.

## 2. SSH access

The deploy keypair is `~/.ssh/hetzner_bench` (operator's laptop).

```sh
ssh -i ~/.ssh/hetzner_bench root@<WASMCLOUD_BENCH_HOST_IP>
```

Optional `~/.ssh/config` entry for convenience:

```text
Host wasmcloud-bench
    HostName <WASMCLOUD_BENCH_HOST_IP>
    User root
    IdentityFile ~/.ssh/hetzner_bench
    IdentitiesOnly yes
```

**Authentication policy:**

- Key-only. `PasswordAuthentication no`, `PermitRootLogin prohibit-password`,
  set via the drop-in `/etc/ssh/sshd_config.d/00-bench-host-hardening.conf`.
- The deploy public key is carried into the installed OS automatically
  (`installimage`'s `-t yes` "take over rescue keys" path).
- Host key fingerprints are pinned in `~/.ssh/known_hosts` after install.
  When re-staging, refresh them; `installimage` regenerates host keys.

## 3. OS install (Hetzner rescue → Ubuntu)

Hetzner ships every dedicated server with a Rescue System (PXE-booted
Debian) that exposes the raw disks. Our OS install runs from there.

The local script [`stage-hetzner.sh`](./stage-hetzner.sh) orchestrates:

```sh
./scripts/bench/stage-hetzner.sh
```

It:

1. Asserts `hostname == rescue` over SSH.
2. Copies [`hetzner-autosetup.cfg`](./hetzner-autosetup.cfg) and
   [`hetzner-postinstall.sh`](./hetzner-postinstall.sh) to the rescue.
3. Prompts for the literal string `WIPE` (the install destroys both disks).
4. Runs Hetzner's `installimage` in batch mode:
   ```sh
   /root/.oldroot/nfs/install/installimage -a -c /autosetup -x /tmp/postinstall.sh
   ```
5. Reboots into the freshly-installed system.

**Autosetup config** ([`hetzner-autosetup.cfg`](./hetzner-autosetup.cfg)):

```text
DRIVE1     /dev/nvme0n1
DRIVE2     /dev/nvme1n1
SWRAID     1
SWRAIDLEVEL 1
BOOTLOADER grub
HOSTNAME   <WASMCLOUD_BENCH_HOSTNAME>
PART       /boot ext4 1024M
PART       /     ext4 all
IMAGE      /root/.oldroot/nfs/install/../images/Ubuntu-2404-noble-amd64-base.tar.gz
```

Layout deliberately:

- **mdraid RAID1** mirrors both NVMes — disk loss should not lose bench
  data while a run is in flight (every result also lands in S3).
- **No swap** — eliminates a known source of runtime jitter.

After install, host-key fingerprints change (`installimage` regenerates
them). Refresh:

```bash
ssh-keygen -R <WASMCLOUD_BENCH_HOST_IP>
ssh-keyscan -t rsa,ecdsa,ed25519 <WASMCLOUD_BENCH_HOST_IP> >> ~/.ssh/known_hosts
```

## 4. Bench-time kernel/cpufreq tweaks

Applied in the install chroot by [`hetzner-postinstall.sh`](./hetzner-postinstall.sh)
so they survive every reboot. Three tweaks, with the rationale:

### 4.1 `nosmt` on the kernel cmdline

Disables hyper-threading. The host comes online with **6 physical
cores** instead of 12 logical CPUs.

**Why:** SMT-pair scheduling jitter is one of the largest sources of
variance in latency benches. Two threads sharing a core's L1/μop cache
will see each other's cache pressure depending on what else the kernel
schedules on the sibling, which drives p99 spikes. With SMT off,
each criterion iteration runs on a fully-owned core and per-iteration
variance drops markedly (we observed std-dev for `hot_invocation` go
from ~14 µs to ~500 ns after this single change).

**How it's wired:** Hetzner's `/etc/default/grub.d/hetzner.cfg`
unconditionally rewrites `GRUB_CMDLINE_LINUX_DEFAULT="consoleblank=0"`,
which would clobber a direct edit to `/etc/default/grub`. So we drop a
`zz-bench.cfg` that sorts after Hetzner's file and *appends* `nosmt`:

```sh
GRUB_CMDLINE_LINUX_DEFAULT="${GRUB_CMDLINE_LINUX_DEFAULT} nosmt"
```

Verify with `nproc` (expect `6`) and `grep nosmt /proc/cmdline`.

### 4.2 `isolcpus=5 nohz_full=5 rcu_nocbs=5` on the kernel cmdline

Reserves CPU 5 from the general scheduler. Pair with `taskset -c 5` to
pin a bench process to it.

**Why:** even with `nosmt` and the performance governor, the kernel can
still preempt a bench thread for IRQ handling, RCU callbacks, or the
periodic scheduler tick. For `gungraun` (deterministic instruction
counts via valgrind/cachegrind) preemption doesn't affect the *result*,
but it bloats runtime by ~2–3×. For wall-clock benches that we may pin
later, scheduler interference is a direct source of p99 variance.

The three flags together give us:

- `isolcpus=5` removes CPU 5 from the general SMP load-balancer. Nothing
  is scheduled there unless explicitly asked.
- `nohz_full=5` stops the periodic scheduler tick on CPU 5 when only
  one runnable task is on it. Kills timer-driven jitter.
- `rcu_nocbs=5` offloads RCU callbacks off CPU 5 to a kthread on the
  housekeeping cores.

CPU index `5` is the last with `nosmt` (CPUs 0..5). Reserving the
trailing CPU leaves 5 cores for the runner agent + system, which is
plenty.

**How it's used:** [`run-bench.sh`](./run-bench.sh) wraps `gungraun`
(and only `gungraun` — the criterion benches are multi-threaded and
would lose throughput) in `taskset -c 5`. The criterion benches run
unpinned across CPUs 0–4. Override the CPU index via `WASMCLOUD_BENCH_ISOLATED_CPU=`
if the host was staged with a different reservation.

**How it's wired:** the same `zz-bench.cfg` GRUB drop-in that appends
`nosmt` also appends the three isolation flags.

Verify:

```sh
cat /sys/devices/system/cpu/isolated      # → 5
cat /proc/cmdline | tr ' ' '\n' | grep -E '^(isolcpus|nohz_full|rcu_nocbs)='
```

**Rollout to an already-staged host:** if `hetzner-postinstall.sh` was
updated after the box was last staged, the drop-in isn't there yet. Run
the Ansible playbook with the `kernel` tag to apply just the GRUB
drop-in idempotently:

```sh
cd scripts/bench/ansible
ansible-playbook provision.yml --tags kernel
ssh -i ~/.ssh/hetzner_bench root@"$WASMCLOUD_BENCH_HOST_IP" reboot
```

The playbook writes the same content `hetzner-postinstall.sh` would
have produced at install time, so the live-patched box and a
fresh-installed box converge on identical state.

### 4.3 cpufreq governor pinned to `performance`

Pins every online CPU's `scaling_governor` to `performance` on boot,
disabling Intel/AMD pstate's "ondemand"-style frequency scaling.

**Why:** `ondemand` ramps clocks based on load, which means a bench
warm-up doesn't actually run at the steady-state frequency the
measured iterations will. Worse, AMD's CPB (Core Performance Boost)
dynamically changes which cores can boost based on thermal headroom,
so different cores hit different peak frequencies. Pinning to
`performance` parks every core at its rated max so warm-up and
measurement are at the same clock.

**How it's wired:** a oneshot systemd unit
`/etc/systemd/system/cpu-performance.service` that runs at boot and
echoes `performance` into every `scaling_governor` sysfs node.

Verify with:
```sh
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor    # → performance
systemctl is-active cpu-performance.service                  # → active
```

### 4.4 No swap

Set in the partition layout (no `PART swap …` line). Avoids OOM-driven
swap-in stalls during bench runs. 62 GiB RAM is plenty for criterion
to run without ever pressuring memory.

### 4.5 What we explicitly do **not** tune

- **NUMA pinning** — single-socket Ryzen, irrelevant.
- **Transparent huge pages, swappiness, dirty ratios** — left at
  Ubuntu defaults. Add deliberately if bench numbers ever justify.
- **CPU C-states** — left at hardware defaults; the `performance`
  governor already pins clocks. Disabling C-states improves p99 at a
  big idle-power cost; not warranted yet.

Document any future tweak in this section AND bake it into the
post-install script (chroot, fresh install) plus the Ansible playbook
(running host) so the next re-stage *and* the next live patch get it
for free.

## 5. Toolchain + dependencies

Run after the OS is up, from your laptop. Provisioning lives in an
Ansible playbook at [`scripts/bench/ansible/`](./ansible/). It's
idempotent — re-run any time to apply drift (new apt deps, a pinned
version bump, a kernel-cmdline change, etc.).

**Prereqs on the laptop:**

```sh
brew install ansible                  # or `pipx install ansible`, your call
brew install --cask 1password-cli     # for the env-helper below

# Populate the env vars from 1Password (item: WASMCLOUD_BENCH_BOX).
# Source, don't execute — the script exports into your current shell.
source scripts/bench/op-env.sh

# SSH_KEY defaults to ~/.ssh/hetzner_bench; override if it lives elsewhere.
```

The helper expects an item called `WASMCLOUD_BENCH_BOX` with fields
`WASMCLOUD_BENCH_HOST_IP`, `WASMCLOUD_BENCH_HOSTNAME`, and (optionally) `WASMCLOUD_BENCH_HOST_IPV6`.
Override the item name with `WASMCLOUD_BENCH_OP_ITEM=<name>` if yours
lives under a different label.

**Run the playbook:**

```sh
cd scripts/bench/ansible
ansible-playbook provision.yml                  # everything
ansible-playbook provision.yml --tags toolchain # apt + rustup + gungraun only
ansible-playbook provision.yml --tags kernel    # just GRUB drop-in
ansible-playbook provision.yml --check          # dry-run; no changes
```

The playbook installs (as `root` on the bench host):

- **apt:** `build-essential pkg-config libssl-dev clang cmake git curl jq
  ca-certificates protobuf-compiler libprotobuf-dev valgrind`
  - `protobuf-compiler` (the `protoc` binary) is needed by
    `crates/wash-runtime/build.rs` for proto-generated bindings.
  - `libprotobuf-dev` provides `/usr/include/google/protobuf/*.proto`
    (the well-known types like `timestamp.proto`); without it,
    `protoc` finds the binary but fails to import.
  - `valgrind` is the measurement engine for the `gungraun`
    instruction-count bench (see §9 and the `gungraun-runner`
    bullet below).
- **rustup** as `root`, with `stable` as the default toolchain. Inside
  the wasmCloud workspace the repo's `rust-toolchain.toml` takes
  precedence (also `stable`, with the `wasm32-unknown-unknown`,
  `wasm32-wasip1`, and `wasm32-wasip2` targets). The default is set so
  out-of-tree `cargo install` invocations (notably
  `gungraun-runner` below) have something to pick.
- **`gungraun-runner`** via `cargo install --version 0.19.1`
  (pinned). gungraun (formerly `iai-callgrind`; renamed at 0.17.0)
  enforces equality between the runner binary and the `gungraun` crate
  version pinned in `crates/wash-runtime/Cargo.toml`; bump them together.
- **Node.js (current LTS)** from NodeSource's apt repo. The CI
  workflow runs `.github/scripts/*.mjs` via `run: node …`, which
  needs an OS-level `node` on `PATH` (self-hosted runners do not
  expose the actions-runner agent's bundled Node to `run:` steps).
  The Ubuntu apt `nodejs` package is on Node 18, which is past EOL,
  so we install via NodeSource and pin the major version in
  `provision.yml` (`node_lts_major`).
- **Repo clone** at `/opt/wasmcloud` (used for ad-hoc/manual benches;
  the CI runner uses its own workspace, see §6).
- A **smoke build**:
  ```sh
  cargo xtask build-fixtures   # benches include_bytes! the wasm fixtures
  cargo bench -p wash-runtime --features wasip3 --bench http_invoke --no-run
  ```
  to verify the bench binary compiles end-to-end. ~3 minutes cold.
  (`run-bench.sh` runs `cargo xtask build-fixtures` for you; it's only
  needed by hand for manual `cargo bench` invocations like this one.)

Verify after the script:

```sh
nproc                              # 6
uname -r                           # 6.8.0-* (Ubuntu Noble)
cargo --version                    # rustup-pinned stable
protoc --version                   # libprotoc 3.21.x or newer
valgrind --version                 # valgrind-3.22.x or newer
gungraun-runner --version          # 0.19.1
node --version                     # the node_lts_major pinned in provision.yml
cat /etc/wasmcloud-bench-stage     # post-install marker
```

## 6. Self-hosted GitHub Actions runner

The CI pipeline runs on a self-hosted runner registered to
`wasmCloud/wasmCloud` with the labels `self-hosted, bench, hetzner`.

Setup is a one-shot. The 1-hour expiry only governs the window between
"obtain a registration token" and "`config.sh` consumes it". Once
`config.sh` finishes, GitHub exchanges that token for long-lived,
auto-rotating runner credentials in `/opt/actions-runner/.credentials`,
and the runner never needs a registration token again unless it's
deregistered or the host is re-staged. See §6.1.

1. Generate a registration token. Either:

   - **GitHub UI:** Settings → Actions → Runners → New self-hosted runner.
   - **gh CLI** (operator-side; see §6.1 below).

2. From your laptop, push it to the host (token via env, not CLI flag,
   so it never lands in `/proc/<pid>/cmdline`):

   ```sh
   ssh -i ~/.ssh/hetzner_bench root@<WASMCLOUD_BENCH_HOST_IP> \
     'cd /opt/wasmcloud && \
      sudo WASMCLOUD_BENCH_RUNNER_TOKEN=<TOKEN> bash scripts/bench/install-runner.sh'
   ```

[`install-runner.sh`](./install-runner.sh) is idempotent (bails if
`/opt/actions-runner` already exists; remove it manually to re-register
with a fresh token). It:

- Creates a non-root **`bench`** user (no shell login). The runner
  process runs as `bench` via systemd.
- Installs **AWS CLI v2** (from the official zip — apt's `awscli`
  package is v1) and `zstd`.
- Installs **rustup as the `bench` user** so `cargo` is available in
  the workflow's PATH. The actual toolchain is auto-installed on
  first build via `rust-toolchain.toml`.
- Downloads `actions-runner-linux-x64-2.334.0.tar.gz`, **verifies the
  SHA256** (constant in the script — bump version + sha together
  when upgrading), extracts to `/opt/actions-runner`.
- Registers the runner with labels `self-hosted, bench, hetzner` and
  name `<WASMCLOUD_BENCH_HOSTNAME>`.
- Installs the runner as a systemd service (`./svc.sh install bench`)
  that starts on boot.

**Persistent paths (owned by `bench`):**

| Path                    | Purpose                                                                                                                                                  |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/var/lib/bench`        | `bench` user's home (where rustup installs cargo)                                                                                                        |
| `/var/lib/bench/work`   | `_work` dir GitHub uses for workflow runs                                                                                                                |
| `/var/lib/bench/target` | `$CARGO_TARGET_DIR` set in the workflow — kept outside the workspace so `actions/checkout`'s `clean` step doesn't blow away the cargo cache between runs |
| `/opt/actions-runner`   | runner binary + config                                                                                                                                   |

**Runner upgrade procedure:**

1. Bump `RUNNER_VERSION` and `RUNNER_SHA256` in
   [`install-runner.sh`](./install-runner.sh) (SHA256 from the runner
   release page).
2. On the host, stop the service, run `./config.sh remove --token …`
   (use a fresh removal token from GitHub), `rm -rf /opt/actions-runner`.
3. Re-run `install-runner.sh` with a fresh registration token.

### 6.1 Token generation via gh CLI (operator-triggered)

Instead of clicking through the GitHub UI, an operator with admin on
`wasmCloud/wasmCloud` can mint a fresh registration token from a local
shell. This is a write API call (it creates a credential), so it's an
**operator-side, manually-invoked** action — Claude/automation does not
run this.

```sh
TOKEN=$(gh api -X POST \
  /repos/wasmCloud/wasmCloud/actions/runners/registration-token \
  --jq '.token')

ssh -i ~/.ssh/hetzner_bench root@<WASMCLOUD_BENCH_HOST_IP> \
  "WASMCLOUD_BENCH_RUNNER_TOKEN=$TOKEN bash -s" \
  < scripts/bench/install-runner.sh
```

Requirements on the operator's laptop:

- `gh` authenticated as a user with admin on the repo, **or** a
  fine-grained PAT / GitHub App credential with `Administration: write`.
- `WASMCLOUD_BENCH_HOSTNAME` and `<WASMCLOUD_BENCH_HOST_IP>` exported from 1Password
  (item: *wasmCloud Bench Host (Hetzner)*).

The token is single-use and short-lived (~1 hour). It flows: gh API →
local shell variable → SSH env → `config.sh` on the host. It never
touches the repo, a CI log, or `/proc/<pid>/cmdline` (env-only).

## 7. Pipeline architecture

```text
┌──────────────────────┐               ┌────────────────────────────────────┐
│  GitHub Actions      │               │   <WASMCLOUD_BENCH_HOSTNAME>       │
│  workflow_dispatch ──┼──────────────▶│   self-hosted runner (user: bench) │
│  inputs: bench, ref  │               │   labels: self-hosted,bench,hetzner│
└──────────────────────┘               │                                    │
        ▲                              │   1. checkout + preflight checks   │
        │ artifact + step              │   2. cargo bench                   │
        │ summary                      │   3. summarize → step summary      │
        │                              │   4. upload criterion artifact     │
        ▼                              │   5. push S3 + invalidate CF       │
┌──────────────────────────────┐       └────────────────────────────────────┘
│ S3 (private)                 │                                  │
│   runs/<date>/<sha>/<run>/…  │◀─────────────────────────────────┘
│   history.json               │ ◄── public via CloudFront ───────────────┐
└──────────────────────────────┘                                          │
                                                                          ▼
                                             ┌──────────────────────────────────┐
                                             │  arewefastyet                    │
                                             │ (separate repo, GitHub Pages)    │
                                             │ fetches history.json             │
                                             └──────────────────────────────────┘
```

**Workflow:** [`.github/workflows/bench.yml`](../../.github/workflows/bench.yml)

- Trigger: `workflow_dispatch` only. Inputs: `bench` (choice of
  `http_invoke` / `wasmtime_baseline` / `wasmtime_serve`) and `ref`
  (any branch/tag/sha; defaults to the workflow's ref).
- `runs-on: [self-hosted, bench, hetzner]`
- `concurrency: bench-host`, `cancel-in-progress: false` — queues
  rather than cancelling, so an in-flight bench is never interrupted.
- `permissions:` minimal; the bench job adds `contents: read` and
  `id-token: write` (OIDC for AWS).
- `CARGO_TARGET_DIR=/var/lib/bench/target` so the cache survives
  `actions/checkout`'s clean.
- `persist-credentials: false` on checkout — the runner runs untrusted
  user code (the bench under test) and we don't want it to read the
  GITHUB_TOKEN.

**Steps, in order:**

1. **Pre-flight** ([`bench-preflight.mjs`](../../.github/scripts/bench-preflight.mjs)) — see §10.
2. **Run bench** ([`run-bench.sh`](./run-bench.sh)) — sources cargo,
   runs `cargo bench -p wash-runtime --features wasip3 --bench <name>`,
   tees output to a per-run log under `$CARGO_TARGET_DIR`.
3. **Summary** (`cargo run -p bench-tools -- summary --bench <name>`) —
   emits a markdown table to `$GITHUB_STEP_SUMMARY` with one row per
   `(group, param)`. Unit semantics live in
   [`crates/bench-tools/src/markdown.rs`](../../crates/bench-tools/src/markdown.rs):
   RPS for batch-throughput benches, B/s for byte throughput, time
   otherwise. For `gungraun` the same subcommand emits an
   instruction-count table from the gungraun output instead.
4. **Upload artifact** — the criterion and/or gungraun output dirs as
   `bench-<bench>-<run-id>` with 90-day retention.
5. **Configure AWS** — `aws-actions/configure-aws-credentials@v6.1.1`
   assumes the `WASMCLOUD_BENCH_AWS_ROLE_ARN` role via OIDC.
6. **Push S3 + invalidate CloudFront**
   ([`bench-push-results.mjs`](../../.github/scripts/bench-push-results.mjs)) —
   uploads per-run artifacts under `runs/…`, then read-modify-writes
   `s3://<bucket>/history.json` (the public aggregate), then issues
   `cloudfront create-invalidation /history.json`.

**Triggers:** `workflow_dispatch` plus `release: published` (the latter
auto-populates the releases timeline; see §9.3). Both require repo
write to fire, which keeps untrusted fork code off the self-hosted
bench host. The `release` matrix is intentionally narrower than the
dispatch choice list — see §9.3.

**Why no `pull_request_target` trigger:** see §9.4. Self-hosted runners
on a public repo are a foot-gun if exposed to fork PRs (a fork can
ship a malicious workflow file or build script to your bench host).
Comparison runs use `workflow_dispatch` only.

## 8. AWS bootstrap (S3 + CloudFront + IAM)

One-shot from a workstation authenticated to the AWS account that
should own the bench bucket:

```sh
./scripts/bench/aws/setup-aws.sh \
  --bucket <bucket> \
  --region <region>
```

[`aws/setup-aws.sh`](./aws/setup-aws.sh) is idempotent. It provisions:

| Resource                                   | Detail                                                                                                                                                                                                   |
| ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **S3 bucket**                              | versioning + AES-256 encryption + public access **fully blocked** + CORS allowing `GET *`                                                                                                                |
| **GitHub OIDC provider**                   | `token.actions.githubusercontent.com` with audience `sts.amazonaws.com`                                                                                                                                  |
| **CloudFront Origin Access Control (OAC)** | sigv4, S3 origin type — replaces the legacy "Origin Access Identity" pattern                                                                                                                             |
| **CloudFront distribution**                | comment `"arewefastyet bench data"`; managed cache policy `CachingOptimized` (respects origin `Cache-Control`); `PriceClass_100` (NA + EU); HTTP/2 + HTTP/3; default `*.cloudfront.net` cert             |
| **Bucket policy**                          | scopes `s3:GetObject` on `history.json` to the **specific** distribution principal via `AWS:SourceArn` — the bucket itself is never reachable directly                                                   |
| **IAM WRITE role**                         | trust pinned to `repo:wasmCloud/wasmCloud:*`; perms: `s3:PutObject` on `runs/*`, `s3:GetObject` + `s3:PutObject` on `history.json`, `s3:ListBucket`, `cloudfront:CreateInvalidation` on the distribution |

The script prints the values to set as repo secrets/vars at the end:

| Repo                              | Setting                                                                                      | Type         |
| --------------------------------- | -------------------------------------------------------------------------------------------- | ------------ |
| `wasmCloud/wasmCloud` (this repo) | `WASMCLOUD_BENCH_AWS_ROLE_ARN`                                                               | secret       |
|                                   | `WASMCLOUD_BENCH_S3_BUCKET`                                                                  | secret       |
|                                   | `WASMCLOUD_BENCH_S3_REGION`                                                                  | secret       |
|                                   | `WASMCLOUD_BENCH_CF_DISTRIBUTION_ID`                                                         | secret       |
|                                   | `WASMCLOUD_BENCH_HOSTNAME` (expected bench host hostname; consumed by `bench-preflight.mjs`) | repo **var** |
| `wasmCloud/arewefastyet` (site)   | `DATA_URL` (e.g. `https://dXXXX.cloudfront.net/history.json`)                                | repo **var** |

`WASMCLOUD_BENCH_HOSTNAME` is set as a **variable** (not a secret) because it's not
strictly sensitive; keeping it out of the repo source is a
defense-in-depth measure rather than a confidentiality requirement.

CloudFront's permanent **Always Free tier** (1 TB egress + 10 M requests
per month) makes data-transfer cost a non-issue for any plausible
dashboard traffic. Storage is ~$0.0001/month for our object sizes.

The site (`wasmCloud/arewefastyet`) reads anonymously through
CloudFront; it has no AWS auth, no IAM role, no AWS secrets.

## 9. Running a bench

### 9.1 Via GitHub (the normal path)

GitHub → **Actions** → **bench** → **Run workflow**:

| Input   | Description                                                                                | Default          |
| ------- | ------------------------------------------------------------------------------------------ | ---------------- |
| `bench` | which bench to run (`http_invoke`, `gungraun`, `wasmtime_baseline`, `wasmtime_serve`) | `http_invoke`    |
| `ref`   | git ref to bench (branch, tag, or sha)                                                     | the workflow ref |

**Bench types:**

| Bench               | Harness       | Measures                           | Notes                                                                                                                                                          |
| ------------------- | ------------- | ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `http_invoke`       | criterion     | wall-clock (ns / req/s)            | wash-runtime HTTP path; cold + hot invocations                                                                                                                 |
| `gungraun`          | gungraun      | CPU instruction count (cachegrind) | deterministic regression detection; not subject to shared-runner timing noise. `gungraun` is the renamed/refreshed `iai-callgrind` (rename landed upstream at 0.17.0) |
| `wasmtime_baseline` | criterion     | wall-clock                         | wasmtime-only baseline for context                                                                                                                             |
| `wasmtime_serve`    | criterion     | wall-clock                         | wasmtime serve subcommand baseline                                                                                                                             |

Anyone with repo-write can dispatch. The job queues on the
`bench-host` concurrency group, so two dispatched runs serialize.

### 9.2 Via SSH (manual / debugging)

Useful for "I just want to see what cargo says without rounding through
GitHub":

```sh
ssh -i ~/.ssh/hetzner_bench root@<WASMCLOUD_BENCH_HOST_IP> \
  '. $HOME/.cargo/env && cd /opt/wasmcloud && \
   cargo xtask build-fixtures && \
   cargo bench -p wash-runtime --features wasip3 --bench http_invoke'
```

Manual runs do **not** touch S3 or arewefastyet — only the GitHub
pipeline writes there. The `/opt/wasmcloud` checkout is cloned by the
Ansible playbook and used for these one-offs (it's separate from the
runner's `_work/` workspace).

To pull manual results back to your laptop:

```sh
rsync -e 'ssh -i ~/.ssh/hetzner_bench' -avz \
  root@<WASMCLOUD_BENCH_HOST_IP>:/opt/wasmcloud/target/criterion/ \
  ./bench-results/$(date -u +%Y-%m-%dT%H%M%SZ)/
```

### 9.3 Adding a release to the trends timeline

To populate the **releases** view on arewefastyet, dispatch with
`ref: vX.Y.Z`. The bench workflow checks out that exact tag, and the
resulting JSONL row carries `ref="vX.Y.Z"`, which the site filters and
renders semver-ordered. No special tooling — just dispatch with the
tag.

Tags also auto-bench on `release: published`: every published release
triggers a matrix run across all four benches against the release tag,
so the timeline self-populates without manual dispatch.

### 9.4 Comparison runs (did this PR move the numbers?)

Separate workflow:
[`.github/workflows/bench-compare.yml`](../../.github/workflows/bench-compare.yml).
Pairs the same bench against two refs back-to-back on the bench host
and produces a delta.

**One entry point:** `workflow_dispatch`. Actions → bench-compare → Run
workflow; supply:

- `bench` — which bench to compare (default `gungraun`).
- `ref_a` — baseline ref (default `main`).
- `ref_b` — candidate ref. To compare a PR, paste the PR's head sha
  here (the PR page shows it; `gh pr view <N> --json headRefOid` also
  works).

**Variance handling:**

- `gungraun`: **1×** per ref. Instruction counts via valgrind are
  deterministic, so repeats don't reduce noise.
- Criterion benches (`http_invoke`, `wasmtime_*`): **3× interleaved**
  (a₁ b₁ a₂ b₂ a₃ b₃); the median of the three is what the delta is
  computed from. ~30 min per bench.

Output: a markdown delta table on the run's step summary. Comparison
runs do **not** push to S3 or update `history.json`, and they do
**not** post a comment back to any PR — they are ephemeral by design.

**Why no PR-label trigger (auto-bench a PR on `bench:run`):**

The bench host is a self-hosted runner. `pull_request_target` gives
the workflow access to base-repo secrets and the self-hosted runner
even when the PR is from a fork; the bench host then has to
`git checkout` the PR head to run the bench, at which point
`run-bench.sh`, fixtures, build scripts, and dependency proc-macros
all come from the PR. Even gated on `author_association`, that's a
higher trust bar than we want — every member's PR ends up running
arbitrary code on the bench host with the same trust as merged code.

`workflow_dispatch` requires the dispatcher to read the diff first
and explicitly opt in, which is the trust model we want for this
particular runner. The bench job has no AWS permissions and no
`pull-requests: write` permission, so a malicious comparison ref
cannot exfiltrate the WRITE role or comment on PRs.

## 10. Pre-flight assertions

[`bench-preflight.mjs`](../../.github/scripts/bench-preflight.mjs) runs first in every CI bench job.
It refuses to proceed if the host has drifted from the baseline,
because measurements on a drifted host are useless and shouldn't end
up on the dashboard.

The script requires the env var `WASMCLOUD_BENCH_HOSTNAME` (the expected hostname
of the dedicated bench host). The workflow sources it from the repo
variable `vars.WASMCLOUD_BENCH_HOSTNAME`; local runs export it from 1Password.
Keeping the value out of the script avoids broadcasting the box's
identity in the public repo.

Hard-checked invariants:

| Check                                | Expected                                               |
| ------------------------------------ | ------------------------------------------------------ |
| `hostname`                           | `${WASMCLOUD_BENCH_HOSTNAME}` (env-injected)           |
| `nproc`                              | `6`  *(SMT off via `nosmt`)*                           |
| `scaling_governor` on every CPU      | `performance`                                          |
| `/sys/devices/system/cpu/isolated`   | `5` *(or `${WASMCLOUD_BENCH_ISOLATED_CPU}`; see §4.2)* |
| `/proc/mdstat` resync                | not in progress                                        |
| 1-min loadavg                        | < 1.0  *(box should be idle)*                          |
| `$CARGO_TARGET_DIR`                  | exists, writable                                       |
| Free space on the target dir's mount | ≥ 5 GiB                                                |
| `cargo` binary                       | `$HOME/.cargo/bin/cargo` exists                        |

If any check fails, the job aborts with a `::error::` annotation
explaining which invariant is violated. Fix the host (or fix the
script if the invariant should be relaxed) and re-dispatch.

## 11. What gets stored where

Per-run uploads (private; only the WRITE role can read):

```text
s3://<bucket>/runs/<YYYY-MM-DD>/<short-sha>/<run-id>/<bench>/
  ├─ criterion.tar.zst   raw criterion data (samples, estimates, SVG reports)
                         only present for criterion-based benches
  ├─ gungraun.tar.zst    raw gungraun output (summary.json, callgrind.out, …)
                         only present for the gungraun bench
  ├─ results.jsonl       one JSON row per (group, param, metric) for trend tools
  ├─ metadata.json       run-level facts (git, host, kernel, timestamps, run URL)
  └─ run.log             cargo bench stdout/stderr
```

Aggregate (publicly readable through CloudFront only — S3 itself is
not reachable):

```text
s3://<bucket>/history.json
  - JSON array of every (group, param, metric) row from every run.
  - Deduped by (sha, bench, group, param, run_attempt, metric), sorted by timestamp.
  - Cache-Control: max-age=60.
  - CloudFront invalidation issued after each push.
  - Capped to last 365 days by HISTORY_MAX_AGE_DAYS.
```

`bench-push-results.mjs` does the read-modify-write on `history.json`
after each run. This is safe without locking because the workflow is
`concurrency: bench-host` — there's only ever one writer.

**Row schema.** Every row carries `metric` (the measurement name) and
`value` (the measurement). Criterion rows additionally carry the
original sibling statistics so older renderers keep working through
the schema bump; gungraun rows carry only `metric`/`value` because the
callgrind summary line doesn't produce per-iteration CIs.

Criterion row (`metric: "mean_ns"`):

```json
{
  "bench": "http_invoke",
  "group": "cold_invocation",
  "param": "p2",
  "sha": "...", "short_sha": "...", "ref": "main",
  "run_id": "...", "run_attempt": "1",
  "timestamp": "2026-05-08T17:21:20Z",
  "host": "<WASMCLOUD_BENCH_HOSTNAME>",
  "kernel": "6.8.0-100-generic",
  "cpus_online": 6,
  "metric": "mean_ns",
  "value": 78667741.74,
  "throughput": {"Elements": 256},
  "mean_ns": 78667741.74,
  "median_ns": 77841173.62,
  "std_dev_ns": 1770558.62,
  "ci_low_ns": 77764094.17,
  "ci_high_ns": 79841655.92
}
```

gungraun row (`metric: "Ir"`):

```json
{
  "bench": "gungraun",
  "group": "http",
  "param": "iaps_hot_invocation.p2",
  "sha": "...", "short_sha": "...", "ref": "main",
  "run_id": "...", "run_attempt": "1",
  "timestamp": "2026-05-08T17:21:20Z",
  "host": "<WASMCLOUD_BENCH_HOSTNAME>",
  "kernel": "6.8.0-100-generic",
  "cpus_online": 6,
  "metric": "Ir",
  "value": 12345678
}
```

The `throughput` field on criterion rows is captured from criterion's
`benchmark.json` and tells the renderer whether to display this row as
time (latency-style) or as ops/sec (throughput-style). gungraun rows have no
throughput field — instruction counts are unit-less in that sense.

## 12. Re-staging from scratch

If the host is hosed (kernel panic, disk corruption, want to bump the
base OS, want to relocate to a different box):

1. **Re-arm Hetzner Rescue** in Hetzner Robot, reboot.
2. **Refresh local host keys** — rescue's keys differ from the installed system's:
   ```sh
   ssh-keygen -R <WASMCLOUD_BENCH_HOST_IP>
   ssh-keyscan -t rsa,ecdsa,ed25519 <WASMCLOUD_BENCH_HOST_IP> >> ~/.ssh/known_hosts
   ```
   (Verify fingerprints against the rescue's MOTD.)
3. **Stage:** `./scripts/bench/stage-hetzner.sh`. After reboot, refresh
   host keys *again* — `installimage` regenerated them.
4. **Provision:** `cd scripts/bench/ansible && ansible-playbook provision.yml` (see §5).
5. **Re-register the runner:** generate a fresh registration token,
   then `sudo WASMCLOUD_BENCH_RUNNER_TOKEN=<X> bash scripts/bench/install-runner.sh`.
6. **GitHub side:** in repo Settings → Actions → Runners, remove the
   old offline runner entry.

No state worth preserving lives on the box — bench results are already
in S3.

## 13. Troubleshooting

### Runner is stuck "offline" in GitHub

```sh
ssh -i ~/.ssh/hetzner_bench root@<WASMCLOUD_BENCH_HOST_IP> \
  'systemctl status actions.runner.* | head -30'
```

If the service crashed: `journalctl -u actions.runner.* -n 100`.
Common cause is GitHub revoked the registration; re-register with a
fresh token.

### `cargo bench` is missing `protoc` or proto includes

Re-run the Ansible playbook (`cd scripts/bench/ansible && ansible-playbook
provision.yml --tags toolchain,apt`); it installs `protobuf-compiler` +
`libprotobuf-dev`. The build needs both — the binary alone fails when
`.proto` files import `google/protobuf/timestamp.proto`.

### `nproc` returns 12 instead of 6

`nosmt` didn't apply. Check:

```sh
cat /proc/cmdline                           # should contain "nosmt"
ls /etc/default/grub.d/zz-bench.cfg         # should exist
update-grub && reboot
```
Hetzner's `/etc/default/grub.d/hetzner.cfg` resets `GRUB_CMDLINE_LINUX_DEFAULT`,
which is why we ship our setting in a `zz-`-prefixed drop-in that sorts
*after* it (see §4.1).

### Governor isn't `performance`

```sh
systemctl status cpu-performance.service
for f in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do cat "$f"; done
```
If the service is enabled but not active, `systemctl start cpu-performance.service`.
If sysfs writes fail, the CPU's cpufreq driver isn't the expected one
(verify `cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_driver`).

### mdraid is resyncing

```sh
cat /proc/mdstat
```
Resync runs after every fresh install (~30 min on these disks). Wait
it out before benching — the I/O contention will skew numbers. The
preflight script already refuses to run during a resync.

### `bench-push-results.mjs` fails with `AccessDenied`

The most common cause is the WRITE role policy missing one of the
permissions added later (e.g., `cloudfront:CreateInvalidation`).
Re-run `./scripts/bench/aws/setup-aws.sh --bucket … --region …` —
it's idempotent and will reconcile the inline policy.

### Site shows stale data

Two-minute lag is normal (`Cache-Control: max-age=60` + browser
cache). For longer staleness:
1. Verify the bench job's `push to s3 + invalidate CloudFront` step
   logged the invalidation ID.
2. Check the distribution: `aws cloudfront list-invalidations --distribution-id <id>`.
3. As a last resort, force-refresh: `aws s3 cp s3://<bucket>/history.json /dev/null`
   then `aws cloudfront create-invalidation --paths "/history.json"`.

## 14. Files in this directory

| File                                                                                                           | Purpose                                                                                                                                                                                                                   |
| -------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`README.md`](./README.md)                                                                                     | This runbook                                                                                                                                                                                                              |
| [`hetzner-autosetup.cfg.tmpl`](./hetzner-autosetup.cfg.tmpl)                                                   | `installimage` config template — `__BENCH_HOSTNAME__` substituted by stage-hetzner.sh at upload time                                                                                                                      |
| [`hetzner-postinstall.sh`](./hetzner-postinstall.sh)                                                           | Chroot hook: `nosmt` + perf governor                                                                                                                                                                                      |
| [`op-env.sh`](./op-env.sh)                                                                                     | `source`-only helper: pulls `WASMCLOUD_BENCH_HOST_IP` / `WASMCLOUD_BENCH_HOSTNAME` / `WASMCLOUD_BENCH_HOST_IPV6` from the 1Password item `WASMCLOUD_BENCH_BOX` into the current shell                                     |
| [`stage-hetzner.sh`](./stage-hetzner.sh)                                                                       | Phase 1 from your laptop: rescue → installed OS                                                                                                                                                                           |
| [`ansible/`](./ansible/)                                                                                       | Phase 2 from your laptop: deps + Rust + valgrind + gungraun-runner + repo + kernel cmdline + perf governor. Idempotent; re-run to apply drift or as the "patch a live box without re-staging" path (`--tags kernel`) |
| [`install-runner.sh`](./install-runner.sh)                                                                     | Phase 3 on the host: GH Actions runner under `bench` user; also installs gungraun-runner. Kept as bash because runner registration takes a one-shot token whose lifecycle is awkward to model declaratively.             |
| [`../../.github/scripts/bench-preflight.mjs`](../../.github/scripts/bench-preflight.mjs)                       | CI step: refuses to bench on a drifted host (env: `WASMCLOUD_BENCH_HOSTNAME`). GHA-only — runs via `node` from the workflow.                                                                                              |
| [`run-bench.sh`](./run-bench.sh)                                                                               | CI step + local: invokes `cargo bench`, writes a run log. Stays bash because compare-bench.sh invokes it on the host for local operator runs.                                                                             |
| [`../../.github/scripts/bench-push-results.mjs`](../../.github/scripts/bench-push-results.mjs)                 | CI step: per-run upload + history.json aggregate + CloudFront invalidate; archives `target/criterion/` and/or `target/gungraun/`. GHA-only.                                                                                    |
| [`compare-bench.sh`](./compare-bench.sh)                                                                       | Pair-bench script: runs one bench against two refs (interleaved for criterion, 1× for gungraun) and snapshots per-iteration data                                                                                               |
| [`../../crates/bench-tools/`](../../crates/bench-tools/)                                                       | Rust binary that parses criterion + gungraun output and renders the JSONL trend rows, the step-summary markdown, and comparison deltas — see "Data processing" below                                                     |
| [`build-history.sh`](./build-history.sh)                                                                       | Maintenance: rebuild `history.json` from scratch by scanning all per-run JSONL in S3                                                                                                                                      |
| [`aws/setup-aws.sh`](./aws/setup-aws.sh)                                                                       | One-shot: bucket + OAC + CloudFront + WRITE role                                                                                                                                                                          |
| [`../../.github/workflows/bench.yml`](../../.github/workflows/bench.yml)                                       | Trends pipeline (workflow_dispatch + release auto-trigger)                                                                                                                                                                |
| [`../../.github/workflows/bench-compare.yml`](../../.github/workflows/bench-compare.yml)                       | Comparison pipeline (workflow_dispatch only — see §9.4 for the rationale against a PR-label trigger)                                                                                                                      |
| [`../../.github/workflows/bench-host-checks.yml`](../../.github/workflows/bench-host-checks.yml)               | Monthly upstream-version checks (no bench-host involvement; opens / updates / auto-closes a tracking issue per check — see §15)                                                                                           |
| [`../../.github/scripts/bench-check-runner-version.mjs`](../../.github/scripts/bench-check-runner-version.mjs) | Compares `RUNNER_VERSION` in `install-runner.sh` to the latest actions/runner release; runs from bench-host-checks.yml                                                                                                    |

Sensitive values (the bench host's IP, IPv6, and hostname) are kept in
1Password rather than in the repo. See §1 and §6.

### Data processing: `crates/bench-tools`

[`crates/bench-tools`](../../crates/bench-tools/) is the Rust binary
that does the structured-data side of the pipeline. Three subcommands:

| Subcommand                                                     | Used by                                                       |
| -------------------------------------------------------------- | ------------------------------------------------------------- |
| `bench-tools jsonl --bench <name>`                             | `bench-push-results.mjs` (writes `results.jsonl`)             |
| `bench-tools summary --bench <name>`                           | `.github/workflows/bench.yml` (writes `$GITHUB_STEP_SUMMARY`) |
| `bench-tools delta` (reads env vars set by `compare-bench.sh`) | `compare-bench.sh` (writes `delta.md` + stdout)               |

Design boundary: **structured-data parsing + rendering lives in Rust;
process orchestration lives in bash (or `.mjs` for GHA-only steps)**.
`bench-tools` parses criterion's `estimates.json` + `benchmark.json`
and valgrind's `callgrind.out` events/summary lines into typed
`serde` structs, then renders markdown or JSONL out the other side.
The shell/JS callers drive the pipeline (SSH, git checkouts,
`cargo bench`, `aws s3 cp`) — those operations are shell-shaped and
spawning subprocesses stays the right tool.

Build is implicit: each invocation site uses `cargo run -p bench-tools
--quiet -- <subcommand>`. Cargo's incremental build keeps the cost
near zero after the first invocation per run. `compare-bench.sh`
explicitly `cargo build`s the binary *before* it starts switching the
worktree across refs, so the renderer stays constant across the two
sides of a comparison.

## 15. Dependency cadence

Bench hosts have an unusual constraint: most ops advice ("patch aggressively")
trades off against measurement stability. The pinned dependency stack splits
into layers, each with its own cadence.

| Layer                                  | Cadence                                           | Pin location                                                                                       | Notes                                                                                                                                                                                                                  |
| -------------------------------------- | ------------------------------------------------- | -------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ubuntu security patches                | continuous (auto)                                 | `unattended-upgrades` (default-on in Noble)                                                        | Only the `-security` pocket. sshd / openssl / glibc fixes go in continuously; kernel held back.                                                                                                                        |
| Kernel                                 | quarterly, deliberate                             | apt — held back implicitly because we don't run blanket `apt upgrade`                              | Scheduler / cgroup / cpufreq behavior is the measurement substrate. A kernel jump can shift `gungraun` Ir counts and criterion p99 in ways that look like regressions. Annotate the dashboard with the bump date.      |
| glibc / libstdc++                      | with the kernel                                   | apt                                                                                                | Same reason.                                                                                                                                                                                                           |
| valgrind                               | yearly, or when gungraun asks for it              | apt                                                                                                | Major valgrind bumps have historically renamed cachegrind event columns; our `Ir` parser is robust to that, but verify.                                                                                                |
| Rust toolchain                         | rolls with `rust-toolchain.toml` (monthly stable) | repo file                                                                                          | No bench-host-specific pin.                                                                                                                                                                                            |
| `gungraun` crate + runner              | when the bench fails to start, or yearly          | `crates/wash-runtime/Cargo.toml` (single source of truth)                                          | Bump the dep version in `Cargo.toml`; `provision.yml` and `install-runner.sh` both derive the runner version from there at install time. gungraun enforces crate-vs-runner equality at run time.                       |
| Node.js                                | quarterly (with the kernel bump)                  | `provision.yml` (`node_lts_major`)                                                                 | Bump to the current active LTS line. Built-ins-only scripts; Node version changes are usually low-risk.                                                                                                                |
| GitHub Actions runner agent            | monthly check, bump on changelog review           | `install-runner.sh` (`RUNNER_VERSION` + `RUNNER_SHA256`)                                           | Auto-tracked via [`bench-host-checks.yml`](../../.github/workflows/bench-host-checks.yml); see below.                                                                                                                  |
| Action SHA pins (`actions/checkout@…`) | weekly via Dependabot                             | `.github/workflows/*.yml`                                                                          | Low risk — these execute on hosted runners, not the bench host.                                                                                                                                                        |
| AWS CLI v2                             | yearly                                            | `install-runner.sh` (curl from official zip)                                                       | Unpinned; we just pull whatever's current at install time.                                                                                                                                                             |

### Monthly: the actions/runner version check

[`bench-host-checks.yml`](../../.github/workflows/bench-host-checks.yml) runs on
the 1st of each month and on `workflow_dispatch`. Today it has one job —
`runner-version` — which compares the `RUNNER_VERSION` pinned in
[`install-runner.sh`](./install-runner.sh) against the latest release of
[actions/runner](https://github.com/actions/runner/releases). If they differ it
opens a tracking issue with the upstream release notes inline so you can scan
whether the bump needs care (new `config.sh` flags, system-dep changes,
deprecations). Re-runs of the workflow update the same issue in place; the
issue auto-closes once `RUNNER_VERSION` catches up to upstream.

To accept a bump:

1. Note the SHA-256 from the release page's assets — `actions-runner-linux-x64-<version>.tar.gz`.
2. Bump `RUNNER_VERSION` and `RUNNER_SHA256` in `install-runner.sh` together.
3. On the bench host: stop the service, deregister with a fresh removal token, `rm -rf /opt/actions-runner`, re-run `install-runner.sh`. See §6 for the full re-registration flow.

### Quarterly: bench-host maintenance window

Once a quarter (~1 hour), do the bench-host-specific bumps together so the
dashboard shows one annotated step instead of N small ones:

1. Re-source `op-env.sh`.
2. Bump apt: `apt update && apt upgrade` on the bench host. This is where
   kernel + glibc + valgrind move forward.
3. Re-run `ansible-playbook provision.yml` — picks up the `node_lts_major`,
   `gungraun_version` bumps you've staged in the repo.
4. If the kernel cmdline drop-in changed: reboot the host.
5. Verify with `cat /etc/wasmcloud-bench-stage` + the `nproc`/`isolcpus` checks
   from §4.
6. Trigger one of each bench against `main` before and after via
   `workflow_dispatch`; annotate the dashboard with the bump date so the
   inevitable step-change is documented.

## 16. Out of scope

Deliberately not built; document the "why not yet" so the next person
doesn't reinvent or reattempt without context.

- **Auto-trigger on `release: published`** — would let new tags
  populate the `releases` view automatically. Easy add when we want
  it; out for v1 because we're still tuning what to bench per release.
- **Comment-driven dispatch** (e.g. `@rust-timer queue` style) —
  needs a fine-grained PAT or GH App. Worth doing once we want PR
  authors to dispatch their own benches.
- **Lifecycle expiry** on `runs/*/criterion.tar.zst` — to cap
  long-term storage. Add a 180-day rule to the bucket lifecycle when
  storage cost ever shows up on the bill (currently ≪ $0.01/mo).
- **Multi-host fan-out** — bench numbers across architectures
  (aarch64, Apple silicon) or multiple x86_64 baselines. Requires
  rethinking the `concurrency: bench-host` group and the per-row
  schema.
- **Auto-regression alerting** — Slack/issue post when a bench
  regresses by N % with non-overlapping CI vs. baseline. The data
  is in S3 already; the consumer is missing.
- **CPU isolation / RT scheduling** — see §4.4. Add only if numbers
  start showing variance we can't explain.
- **Custom CloudFront domain** (`data.https://wasmcloud.github.io/arewefastyet`) —
  optional polish; would need an ACM cert in `us-east-1` validated
  via Cloudflare DNS, then attached to the distribution.

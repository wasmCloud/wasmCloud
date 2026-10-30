#!/usr/bin/env python3
"""Run isolated guest-memory pressure cases with Docker and Python 3.11+."""

import argparse
import hashlib
import itertools
import json
import pathlib
import subprocess
import sys
import time
import tomllib
import uuid

MIB = 1 << 20
ROOT = pathlib.Path(__file__).resolve().parents[2]
EXAMPLE = "guest_memory_pressure"
SHAPES = ("sparse", "data-heavy", "multiple", "dirty")


def command(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def classify(case, state, samples):
    """Require phase evidence; an exit code alone never proves an OOM."""
    events = [sample.get("event") for sample in samples]
    if "ready" not in events:
        return False, "failed before admission completed"
    ready = samples[events.index("ready")]
    expected = {"mode": case["mode"], "cow": case["cow"],
                "shape": case["shape"], "limit": case["memory_mib"] * MIB,
                "swap_limit": 0,
                "guest_limit": (case["guest_memory_mib"] * MIB
                                if case.get("guest_memory_mib")
                                else max(256 * MIB, case["memory_mib"] * MIB * 3 // 4))}
    if case["mode"] == "growth":
        expected["initial_limit"] = (case["initial_cap_mib"] * MIB
                                     if case.get("initial_cap_mib") else 4 * ready["guest_limit"])
    if any(ready.get(key) != value for key, value in expected.items()):
        return False, "reported settings differ from requested settings"
    if ready.get("admitted", 0) <= 0:
        return False, "no guests admitted"
    memories, per_memory = {"sparse": (1, MIB), "data-heavy": (1, 8 * MIB),
                            "multiple": (4, MIB), "dirty": (1, case["dirty_mib"] * MIB)}[case["shape"]]
    if ready.get("initial_bytes") != memories * per_memory or ready.get("memories") != memories:
        return False, "fixture has the wrong memory shape"
    total = ready["admitted"] * memories * per_memory
    uncharged = ready["admitted"] * memories * min(per_memory, 2 * MIB) if case["mode"] == "growth" else 0
    if ready.get("charged") != total - uncharged or ready.get("uncharged") != uncharged:
        return False, "guest reservations differ from the retained memories"
    if ready["charged"] > ready["guest_limit"] or uncharged > ready["initial_limit"]:
        return False, "guest reservations exceeded their budgets"
    if not (ready.get("process_over") or any(ready.get(key, 0) > 0 for key in
            ("guest_refused", "initial_refused", "process_refused"))):
        return False, "admission stopped without memory-pressure evidence"
    if state.get("OOMKilled"):
        if (case["mode"] == "growth" and case["shape"] == "dirty"
                and events == ["baseline", "ready", "writing"]
                and state.get("ExitCode") == 137
                and ready["admitted"] * ready["initial_bytes"] > ready["limit"]):
            return True, "expected late-write OOM"
        return False, "unexpected OOM (admission or control case)"
    if state.get("ExitCode") != 0:
        return False, f"process exited {state.get('ExitCode')} without verified OOM"
    if events != ["baseline", "ready", "writing", "touched", "released"]:
        return False, "missing or out-of-order completion samples"
    touched, released = samples[-2:]
    if touched.get("admitted") != ready["admitted"]:
        return False, "retained guest count changed before the final sample"
    if any(touched.get(key) != ready[key] for key in ("charged", "uncharged")):
        return False, "reservations changed during writes without memory growth"
    if any(released.get(key) != 0 for key in ("admitted", "charged", "uncharged")):
        return False, "guest accounting did not return to zero"
    if (case["shape"] == "dirty"
            and ready["admitted"] * ready["initial_bytes"] > ready["limit"]):
        return False, "full private writes exceeded the cgroup limit without an OOM"
    return True, "completed; admission bounded retained memory"


def build(args, output):
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    image = f"rust:{toolchain}-bookworm"
    invocation = [
        "docker", "run", "--rm", "--memory=8g", "--memory-swap=8g", "--cpus=4",
        "--mount", f"type=bind,src={ROOT},dst=/work,readonly",
        "--mount", f"type=volume,src={args.target_volume},dst=/target",
        "--mount", f"type=volume,src={args.registry_volume},dst=/usr/local/cargo/registry",
        "--workdir=/work", "--env=CARGO_TARGET_DIR=/target",
        f"--env=RUSTUP_TOOLCHAIN={toolchain}", image,
        "cargo", "build", "--release", "--locked", "--no-default-features",
        "-p", "wash-runtime", "--example", EXAMPLE,
        "--message-format=json-render-diagnostics",
    ]
    print("Building the Linux pressure-test example (first build can take several minutes).", flush=True)
    with (output / "build.jsonl").open("w") as log:
        subprocess.run(invocation, stdout=log, check=True)
    binary = None
    for line in (output / "build.jsonl").read_text().splitlines():
        message = json.loads(line)
        if message.get("target", {}).get("name") == EXAMPLE and message.get("executable"):
            binary = message["executable"]
    if binary is None or not binary.startswith("/target/"):
        raise RuntimeError("cargo did not report the pressure-test executable")
    image_id = command("docker", "image", "inspect", image, "--format={{.Id}}")
    return image_id, binary


def run_case(args, output, image, binary, case):
    name = f"wash-pressure-{uuid.uuid4().hex}"
    label = (f"{case['memory_mib']}m-{case['shape']}-{case['mode']}-"
             f"cow-{str(case['cow']).lower()}-{case['repeat']}")
    log_path = output / f"{label}.jsonl"
    error_path = output / f"{label}.stderr"
    create = [
        "docker", "create", f"--name={name}", "--network=none", "--cpus=2",
        f"--memory={case['memory_mib']}m", f"--memory-swap={case['memory_mib']}m",
        "--mount", f"type=volume,src={args.target_volume},dst=/target,readonly",
        "--env=WASH_PRESSURE_RUN=1", f"--env=WASH_PRESSURE_MODE={case['mode']}",
        f"--env=WASH_PRESSURE_COW={str(case['cow']).lower()}",
        f"--env=WASH_PRESSURE_SHAPE={case['shape']}",
        f"--env=WASH_PRESSURE_INITIAL={case['dirty_mib']}MiB",
    ]
    if args.initial_cap_mib:
        create.append(f"--env=WASH_PRESSURE_INITIAL_CAP={args.initial_cap_mib}MiB")
    if args.guest_memory_mib:
        create.append(f"--env=WASH_PRESSURE_GUEST_CAP={args.guest_memory_mib}MiB")
    create.extend([image, binary])
    started = time.monotonic()
    timed_out = False
    try:
        command(*create)
        with log_path.open("w") as log, error_path.open("w") as error:
            try:
                subprocess.run(["docker", "start", "--attach", name], stdout=log,
                               stderr=error, timeout=args.timeout, check=False)
            except subprocess.TimeoutExpired:
                timed_out = True
                command("docker", "kill", name)
        inspected = json.loads(command("docker", "inspect", name))[0]
        samples = [json.loads(line) for line in log_path.read_text().splitlines() if line.strip()]
        passed, outcome = classify(case, inspected["State"], samples)
        if timed_out:
            passed, outcome = False, "timed out"
        result = {"case": case, "passed": passed, "outcome": outcome,
                  "seconds": round(time.monotonic() - started, 3),
                  "state": inspected["State"], "samples": samples,
                  "container_limits": {key: inspected["HostConfig"][key]
                                       for key in ("Memory", "MemorySwap", "NanoCpus")}}
        (output / f"{label}.json").write_text(json.dumps(result, indent=2) + "\n")
        ready = next((sample for sample in samples if sample["event"] == "ready"), {})
        print(f"{'PASS' if passed else 'FAIL'} {label}: {outcome}; "
              f"admitted={ready.get('admitted', '?')}", flush=True)
        return result
    finally:
        subprocess.run(["docker", "rm", "--force", name], stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, check=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--memory-mib", nargs="+", type=int, default=[768])
    parser.add_argument("--shapes", nargs="+", choices=SHAPES, default=list(SHAPES))
    parser.add_argument("--modes", nargs="+", choices=("enforce", "growth"), default=["enforce", "growth"])
    parser.add_argument("--cow", nargs="+", choices=("on", "off"), default=["on", "off"])
    parser.add_argument("--dirty-mib", type=int, choices=(1, 2), default=1)
    parser.add_argument("--initial-cap-mib", type=int, help="growth-mode initial allowance in MiB")
    parser.add_argument("--guest-memory-mib", type=int, help="charged guest budget in MiB")
    parser.add_argument("--repeats", type=int, default=1)
    parser.add_argument("--timeout", type=int, default=120, help="seconds per measured container")
    parser.add_argument("--target-volume", default="wash-pressure-target")
    parser.add_argument("--registry-volume", default="wash-pressure-registry")
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "target/guest-memory-pressure")
    args = parser.parse_args()
    if any(memory < 384 or memory > 2048 for memory in args.memory_mib):
        parser.error("memory limits must be between 384 and 2048 MiB")
    if args.repeats < 1 or args.timeout < 1:
        parser.error("repeats and timeout must be positive")
    if any(value is not None and value <= 0 for value in (args.initial_cap_mib, args.guest_memory_mib)):
        parser.error("guest memory and initial allowance must be positive")
    output = args.output.resolve() / (time.strftime("%Y%m%d-%H%M%S") + "-" + uuid.uuid4().hex[:8])
    output.mkdir(parents=True)
    print(f"Results: {output}", flush=True)
    image, binary = build(args, output)
    manifest = {
        "revision": command("git", "rev-parse", "HEAD", cwd=ROOT),
        "dirty": bool(command("git", "status", "--porcelain", cwd=ROOT)),
        "example_sha256": hashlib.sha256((ROOT / f"crates/wash-runtime/examples/{EXAMPLE}.rs").read_bytes()).hexdigest(),
        "runner_sha256": hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),
        "image": image, "binary": binary,
        "settings": {key: str(value) if isinstance(value, pathlib.Path) else value
                     for key, value in vars(args).items()},
    }
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    results = []
    for memory, shape, repeat, cow, mode in itertools.product(
            args.memory_mib, args.shapes, range(1, args.repeats + 1), args.cow, args.modes):
        case = {"memory_mib": memory, "shape": shape, "repeat": repeat,
                "cow": cow == "on", "mode": mode, "dirty_mib": args.dirty_mib,
                "initial_cap_mib": args.initial_cap_mib, "guest_memory_mib": args.guest_memory_mib}
        results.append(run_case(args, output, image, binary, case))
        (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    failed = sum(not result["passed"] for result in results)
    print(f"{len(results) - failed}/{len(results)} cases passed; results: {output}")
    return int(failed > 0)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (subprocess.CalledProcessError, OSError, ValueError, RuntimeError) as error:
        sys.exit(str(error))

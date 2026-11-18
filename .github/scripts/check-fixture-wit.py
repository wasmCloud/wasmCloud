#!/usr/bin/env python3
"""Resolve fixture WIT using only committed local overrides, without a registry."""

import argparse
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib


def check_fixture(config: Path, scratch: Path) -> None:
    fixture = config.parent
    wit = scratch / fixture.name
    # Ignore generated deps: an old fetch must not hide a missing local override.
    shutil.copytree(fixture / "wit", wit, ignore=shutil.ignore_patterns("deps"))
    deps = wit / "deps"
    deps.mkdir()
    with config.open("rb") as source:
        overrides = tomllib.load(source).get("overrides", {})
    for package, override in overrides.items():
        if "path" not in override:
            raise ValueError(f"{package} needs a local path override")
        source = (fixture / override["path"]).resolve()
        target = deps / package.replace(":", "-")
        if source.is_dir():
            shutil.copytree(source, target)
        else:
            target.mkdir()
            shutil.copy2(source, target / source.name)
    result = subprocess.run(
        ["wasm-tools", "component", "wit", str(wit)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode:
        raise ValueError(result.stderr.strip())


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "fixtures",
        nargs="?",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "crates/wash-runtime/tests/fixtures",
    )
    fixtures = parser.parse_args().fixtures
    configs = sorted(fixtures.glob("*/wkg.toml"))
    if not configs:
        parser.error(f"no fixture manifests found in {fixtures}")
    failures = 0
    with tempfile.TemporaryDirectory(prefix="fixture-wit-offline-") as scratch:
        for config in configs:
            try:
                check_fixture(config, Path(scratch))
            except (OSError, ValueError) as error:
                print(f"{config.parent.name}: {error}", file=sys.stderr)
                failures += 1
    print(f"Offline WIT resolution: {len(configs) - failures}/{len(configs)} fixtures passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

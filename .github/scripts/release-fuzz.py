#!/usr/bin/env python3
"""Stage trusted fuzz harnesses against a release checkout."""

import argparse
import json
import shutil
import tomllib
from pathlib import Path


PARSER_DEPENDENCIES = ("http", "oci-client", "prost", "serde_json")
FUZZ_ONLY_PACKAGES = {"wash-runtime-fuzz", "libfuzzer-sys", "arbitrary"}
ROUTE_INCLUDE = '''
#[allow(unexpected_cfgs)]
pub mod release_fuzz {
    #[cfg(fuzzing)]
    use super::*;
    #[cfg(fuzzing)]
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fuzz/support/http_route.rs"));
}
'''


def toml_value(value):
    if isinstance(value, dict):
        fields = ", ".join(f"{key} = {toml_value(item)}" for key, item in value.items())
        return "{ " + fields + " }"
    return json.dumps(value)


def prepare(source, harness, language):
    if source == harness:
        raise ValueError("use a separate release checkout for fuzzing")
    if language == "go":
        files = sorted((harness / "runtime-operator").rglob("*_fuzz_test.go"))
        if not files:
            raise ValueError("no Go fuzz harnesses found")
        for file in files:
            destination = source / file.relative_to(harness)
            if not destination.parent.is_dir():
                raise ValueError(f"release is missing Go package {destination.parent}")
            shutil.copyfile(file, destination)
        return

    dependencies = tomllib.loads((source / "Cargo.toml").read_text())["workspace"]["dependencies"]
    manifest = (harness / "fuzz/Cargo.toml").read_text()
    manifest += "\n[workspace.dependencies]\n"
    for name in PARSER_DEPENDENCIES:
        dependency = dependencies[name]
        if isinstance(dependency, dict) and ("path" in dependency or "git" in dependency):
            raise ValueError(f"cannot stage registry parser dependency {name}: {dependency}")
        manifest += f"{name} = {toml_value(dependency)}\n"
    destination = source / "fuzz"
    destination.mkdir(exist_ok=True)
    for directory in ("src", "fuzz_targets", "seeds", "support"):
        shutil.copytree(harness / "fuzz" / directory, destination / directory, dirs_exist_ok=True)
    (destination / "Cargo.toml").write_text(manifest)
    shutil.copyfile(source / "Cargo.lock", destination / "Cargo.lock")
    route_file = source / "crates/wash-runtime/src/host/http.rs"
    routes = route_file.read_text()
    if ROUTE_INCLUDE not in routes:
        route_file.write_text(routes + ROUTE_INCLUDE)


def package_identity(package):
    return tuple(package.get(key) for key in ("name", "version", "source", "checksum"))


def verify_lock(source):
    release = tomllib.loads((source / "Cargo.lock").read_text())["package"]
    fuzz = tomllib.loads((source / "fuzz/Cargo.lock").read_text())["package"]
    locked = {package_identity(package) for package in release}
    names = {package["name"] for package in release}
    for package in fuzz:
        if package["name"] in FUZZ_ONLY_PACKAGES and package["name"] not in names:
            continue
        if package_identity(package) not in locked:
            raise ValueError(
                f"fuzz dependency {package['name']} {package['version']} differs from release Cargo.lock"
            )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "verify-lock"))
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--language", choices=("go", "rust"))
    args = parser.parse_args()
    source = args.source.resolve()
    if args.action == "prepare":
        if not args.language:
            parser.error("prepare requires --language")
        prepare(source, Path(__file__).resolve().parents[2], args.language)
    else:
        verify_lock(source)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Regression tests for release fuzz staging and dependency pinning."""

import importlib.util
import tempfile
import sys
import tomllib
import unittest
from pathlib import Path


sys.dont_write_bytecode = True

SPEC = importlib.util.spec_from_file_location("release_fuzz", Path(__file__).with_name("release-fuzz.py"))
fuzz = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fuzz)
HARNESS = Path(__file__).resolve().parents[2]
LOCK = '''version = 4
[[package]]
name = "serde_json"
version = "1.0.100"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-checksum"
'''


class ReleaseFuzzTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.source = Path(self.temporary.name)
        (self.source / "Cargo.toml").write_text('''[workspace.dependencies]
http = { version = "1", default-features = false }
oci-client = { version = "0.16", default-features = false, features = ["rustls-tls"] }
prost = { version = "0.13", default-features = false }
serde_json = "1"
''')
        (self.source / "Cargo.lock").write_text(LOCK)
        self.routes = self.source / "crates/wash-runtime/src/host/http.rs"
        self.routes.parent.mkdir(parents=True)
        self.routes.write_text("// Release parser implementation.\n")

    def test_recovers_checkout_without_fuzz_files_or_hook(self):
        fuzz.prepare(self.source, HARNESS, "rust")
        generated = self.source / "fuzz"
        manifest = tomllib.loads((generated / "Cargo.toml").read_text())
        dependencies = manifest["workspace"]["dependencies"]
        self.assertEqual(dependencies["oci-client"]["version"], "0.16")
        self.assertEqual(dependencies["prost"]["version"], "0.13")
        self.assertEqual((generated / "Cargo.lock").read_text(), LOCK)
        self.assertTrue((generated / "seeds/http_route/local").is_file())
        self.assertTrue((generated / "support/http_route.rs").is_file())
        self.assertTrue(self.routes.read_text().startswith("// Release parser implementation.\n"))
        fuzz.prepare(self.source, HARNESS, "rust")
        self.assertEqual(self.routes.read_text().count(fuzz.ROUTE_INCLUDE), 1)
        self.assertEqual((self.source / "Cargo.lock").read_text(), LOCK)

    def test_rejects_dependency_drift_and_reseeds_from_release(self):
        fuzz.prepare(self.source, HARNESS, "rust")
        lock = self.source / "fuzz/Cargo.lock"
        lock.write_text(LOCK.replace('version = "1.0.100"', 'version = "1.0.101"'))
        with self.assertRaisesRegex(ValueError, "serde_json 1.0.101 differs"):
            fuzz.verify_lock(self.source)
        fuzz.prepare(self.source, HARNESS, "rust")
        fuzz.verify_lock(self.source)
        (self.source / "Cargo.lock").write_text(LOCK.replace("1.0.100", "1.0.102"))
        fuzz.prepare(self.source, HARNESS, "rust")
        self.assertIn("1.0.102", lock.read_text())
        fuzz.verify_lock(self.source)

    def test_rejects_source_or_checksum_changes_and_new_runtime_dependencies(self):
        fuzz.prepare(self.source, HARNESS, "rust")
        lock = self.source / "fuzz/Cargo.lock"
        for old, new in (("release-checksum", "changed"), ("crates.io-index", "other-index")):
            with self.subTest(new=new):
                lock.write_text(LOCK.replace(old, new))
                with self.assertRaisesRegex(ValueError, "differs from release"):
                    fuzz.verify_lock(self.source)
        lock.write_text(LOCK + '\n[[package]]\nname = "unexpected-runtime-dep"\nversion = "1.0.0"\n')
        with self.assertRaisesRegex(ValueError, "unexpected-runtime-dep"):
            fuzz.verify_lock(self.source)

    def test_allows_fuzz_tooling_without_changing_existing_tooling_dependencies(self):
        fuzz.prepare(self.source, HARNESS, "rust")
        lock = self.source / "fuzz/Cargo.lock"
        lock.write_text(LOCK + '\n[[package]]\nname = "libfuzzer-sys"\nversion = "0.4.10"\n')
        fuzz.verify_lock(self.source)
        arbitrary = '\n[[package]]\nname = "arbitrary"\nversion = "1.4.2"\n'
        (self.source / "Cargo.lock").write_text(LOCK + arbitrary)
        lock.write_text(LOCK + arbitrary.replace("1.4.2", "1.5.0"))
        with self.assertRaisesRegex(ValueError, "arbitrary 1.5.0 differs"):
            fuzz.verify_lock(self.source)

    def test_restores_missing_go_harnesses_without_changing_module(self):
        operator = self.source / "runtime-operator"
        operator.mkdir()
        module = operator / "go.mod"
        module.write_text("module release\n")
        for file in (HARNESS / "runtime-operator").rglob("*_fuzz_test.go"):
            (self.source / file.relative_to(HARNESS)).parent.mkdir(parents=True, exist_ok=True)
        fuzz.prepare(self.source, HARNESS, "go")
        self.assertEqual(module.read_text(), "module release\n")
        self.assertTrue((operator / "api/runtime/v1alpha1/workload_fuzz_test.go").is_file())


if __name__ == "__main__":
    unittest.main()

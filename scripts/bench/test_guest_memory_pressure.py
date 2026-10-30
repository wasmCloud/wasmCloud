"""Check pressure-result classification without Docker or memory pressure."""

import copy
import importlib.util
import pathlib
import unittest

SPEC = importlib.util.spec_from_file_location(
    "pressure", pathlib.Path(__file__).with_name("guest-memory-pressure.py"))
PRESSURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PRESSURE)
MIB = PRESSURE.MIB


class PressureResultTests(unittest.TestCase):
    def setUp(self):
        self.case = {"memory_mib": 768, "shape": "dirty", "mode": "growth",
                     "cow": True, "dirty_mib": 1, "initial_cap_mib": None,
                     "guest_memory_mib": None}
        self.ready = {"event": "ready", "mode": "growth", "shape": "dirty", "cow": True,
                      "limit": 768 * MIB, "swap_limit": 0, "admitted": 2304,
                      "initial_bytes": MIB, "memories": 1, "guest_limit": 576 * MIB,
                      "initial_limit": 2304 * MIB, "charged": 0, "uncharged": 2304 * MIB,
                      "initial_refused": 1}
        self.samples = [{**self.ready, "event": event}
                        for event in ("baseline", "ready", "writing")]
        self.state = {"ExitCode": 137, "OOMKilled": True}

    def classify(self):
        return PRESSURE.classify(self.case, self.state, self.samples)[0]

    def complete(self, admitted):
        for sample in self.samples:
            sample["admitted"] = admitted
            sample["uncharged"] = admitted * MIB
        self.samples += [{**self.samples[-1], "event": "touched"},
                         {**self.samples[-1], "event": "released", "admitted": 0,
                          "charged": 0, "uncharged": 0}]
        self.state = {"ExitCode": 0, "OOMKilled": False}

    def test_verified_late_write_oom_is_expected(self):
        self.assertTrue(self.classify())

    def test_exit_137_alone_is_not_oom_evidence(self):
        self.state["OOMKilled"] = False
        self.assertFalse(self.classify())

    def test_oom_before_writes_is_a_failure(self):
        self.samples.pop()
        self.assertFalse(self.classify())
        self.samples.pop()
        self.assertFalse(self.classify())

    def test_enforce_oom_is_a_failure(self):
        self.case["mode"] = "enforce"
        for sample in self.samples:
            sample.update(mode="enforce", admitted=576, charged=576 * MIB, uncharged=0)
        self.assertFalse(self.classify())

    def test_sparse_oom_is_a_failure(self):
        self.case["shape"] = "sparse"
        for sample in self.samples:
            sample["shape"] = "sparse"
        self.assertFalse(self.classify())

    def test_settings_must_match(self):
        for key, value in (("cow", False), ("swap_limit", MIB),
                           ("limit", 384 * MIB), ("initial_limit", 2048 * MIB)):
            with self.subTest(key=key):
                samples = copy.deepcopy(self.samples)
                samples[1][key] = value
                self.assertFalse(PRESSURE.classify(self.case, self.state, samples)[0])

    def test_bounded_completion_and_release(self):
        self.complete(576)
        self.assertTrue(self.classify())
        self.samples[-1]["uncharged"] = MIB
        self.assertFalse(self.classify())

    def test_missing_final_sample_is_a_failure(self):
        self.complete(576)
        self.samples.pop()
        self.assertFalse(self.classify())

    def test_unbounded_full_write_completion_is_invalid(self):
        self.complete(2304)
        self.assertFalse(self.classify())

    def test_sparse_completion_can_exceed_the_declared_container_size(self):
        self.case["shape"] = "sparse"
        for sample in self.samples:
            sample["shape"] = "sparse"
        self.complete(2304)
        self.assertTrue(self.classify())

    def test_data_heavy_reservations_include_charged_initial_bytes(self):
        self.case["shape"] = "data-heavy"
        self.complete(96)
        for sample in self.samples[:-1]:
            sample.update(shape="data-heavy", initial_bytes=8 * MIB,
                          charged=576 * MIB, uncharged=192 * MIB)
        self.assertTrue(self.classify())

    def test_custom_budget_completion(self):
        self.case.update(initial_cap_mib=288, guest_memory_mib=256)
        for sample in self.samples:
            sample.update(initial_limit=288 * MIB, guest_limit=256 * MIB)
        self.complete(288)
        self.assertTrue(self.classify())

    def test_incorrect_accounting_is_a_failure(self):
        self.samples[1]["uncharged"] -= MIB
        self.assertFalse(self.classify())

    def test_over_budget_reservations_are_a_failure(self):
        for sample in self.samples:
            sample.update(admitted=2305, uncharged=2305 * MIB)
        self.assertFalse(self.classify())

    def test_unrelated_instantiation_failure_is_not_pressure(self):
        self.samples[1]["initial_refused"] = 0
        self.assertFalse(self.classify())


if __name__ == "__main__":
    unittest.main()

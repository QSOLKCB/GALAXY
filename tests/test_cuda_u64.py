#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
import importlib.util
import json
import math
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "runtime" / "cuda" / "galaxy_u64_cuda.py"
SPEC = importlib.util.spec_from_file_location("galaxy_u64_cuda", MODULE_PATH)
cuda = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(cuda)


class CudaU64HostContractTests(unittest.TestCase):
    def test_boundary_fingerprints_match_published_host_contract(self):
        ids = [(1 << 32) - 1, 1 << 32, (1 << 32) + 1]
        self.assertEqual(
            [cuda.address_fingerprint(i, 303) for i in ids],
            [
                "88a9cb0a03f49db32d612a515098933823650020",
                "6551a64769e46bffb7daefac9d10edd3281ad3a1",
                "1ec272fb214e54c8219d9390572850e1bdcb7ce5",
            ],
        )

    def test_canonical_beyond_u32_job_resolves_without_cuda(self):
        job = cuda.load_job(ROOT / "runtime" / "jobs" / "beyond-u32.json")
        task = job["task"]
        self.assertEqual(task["logical_particles"], 4_303_355_904)
        self.assertEqual(task["tile_particles"], 8_388_608)
        self.assertEqual(cuda.tile_count(task), 513)
        self.assertEqual(cuda.particle_updates(task), 4_303_355_904_000)

    def test_unknown_fields_and_boolean_integers_fail_closed(self):
        with self.assertRaisesRegex(ValueError, "Unknown u64 task fields"):
            cuda.validate_job({"schema_version": 1, "task": {"particles": 4}})
        with self.assertRaisesRegex(ValueError, "direction must be an integer"):
            cuda.validate_job({"schema_version": 1, "task": {"direction": True}})

    def test_sample_plan_is_exact_monotonic_and_reaches_high_ids(self):
        job = cuda.validate_job({
            "schema_version": 1,
            "task": {
                "logical_particles": (1 << 32) + 100_000_000,
                "tile_particles": 4096,
                "snapshot_limit": 257,
                "steps": 1,
            },
        })
        ids = cuda.sample_ids(job["task"])
        self.assertEqual(len(ids), 257)
        self.assertEqual(ids, sorted(ids))
        self.assertEqual(len(ids), len(set(ids)))
        self.assertGreater(ids[-1], 1 << 32)

    def test_phase_split_is_finite(self):
        parts = cuda.phase_time_parts(123456789.25)
        self.assertEqual(len(parts), 4)
        self.assertTrue(all(math.isfinite(x) for x in parts))

    def test_png_writer_emits_valid_signature(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "x.png"
            cuda.write_rgb_png(path, 1, 1, bytes([3, 5, 9]))
            self.assertEqual(path.read_bytes()[:8], b"\x89PNG\r\n\x1a\n")


if __name__ == "__main__":
    unittest.main()

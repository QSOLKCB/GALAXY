#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "scripts" / "bench-bh2d-hardware.py"
SPEC = importlib.util.spec_from_file_location("bench_bh2d_hardware", MODULE_PATH)
sweep = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(sweep)


def receipt_for(particles: int, *, steps: int = 3, oracle_limit: int = 4096, repeats: int = 7):
    oracle_status = "executed" if particles <= oracle_limit else "skipped-particle-limit"
    serial = (
        {
            "status": "executed",
            "samples_seconds": [0.01] * repeats,
            "median_seconds": 0.01,
            "parallel_vs_serial_speedup": 2.0,
        }
        if particles <= 4096
        else {"status": "skipped-bh2c-cap", "limit": 4096}
    )
    return {
        "schema": sweep.RECEIPT_SCHEMA,
        "status": "complete",
        "phase": "BH-2D",
        "builder": "gpu-parallel-sparse-radix-v1",
        "measurement_class": "hardware",
        "hardware_performance_claim_allowed": True,
        "particles": particles,
        "steps": steps,
        "gpu": {
            "name": "Synthetic GPU",
            "backend": "Vulkan",
            "device_type": "DiscreteGpu",
            "software": False,
        },
        "host_tree_rebuilds": 0,
        "host_particle_readbacks_during_steps": 0,
        "force_solves": steps + 1,
        "evolution_tree_builds": steps + 1,
        "parallel_tree_buffer_bytes": particles * 1024,
        "tree": {
            "repeat_rebuild_matches": True,
            "active_cell_count": 17,
            "leaf_count": 8,
            "max_depth": 6,
        },
        "final_force": {
            "direct_probe_rms_relative": 0.001,
            "direct_probe_max_relative": 0.01,
        },
        "trajectory_vs_bh2a_flat_f64": {"status": oracle_status},
        "gpu_oracles": {"status": oracle_status},
        "tree_build_benchmark": {
            "stage_timings_are_diagnostics": True,
            "parallel_samples_seconds": [0.005] * repeats,
            "parallel_median_seconds": 0.005,
            "bh2c_serial": serial,
        },
    }


class Bh2dHardwareSweepTests(unittest.TestCase):
    def test_particle_list_must_be_strictly_increasing(self):
        self.assertEqual(sweep.parse_particles("512,4096,8192"), [512, 4096, 8192])
        for invalid in ("", "4096,512", "512,512", "1", "65537", "512,nope"):
            with self.subTest(invalid=invalid):
                with self.assertRaises(sweep.SweepError):
                    sweep.parse_particles(invalid)

    def test_hardware_receipt_at_oracle_size_is_accepted(self):
        summary = sweep.validate_receipt(
            receipt_for(4096),
            particles=4096,
            steps=3,
            oracle_limit=4096,
            benchmark_repeats=7,
        )
        self.assertEqual(summary["particles"], 4096)
        self.assertEqual(summary["bh2c_parallel_vs_serial_speedup"], 2.0)
        self.assertGreater(summary["parallel_median_seconds"], 0.0)

    def test_large_hardware_receipt_requires_explicit_oracle_skips(self):
        summary = sweep.validate_receipt(
            receipt_for(8192),
            particles=8192,
            steps=3,
            oracle_limit=4096,
            benchmark_repeats=7,
        )
        self.assertEqual(summary["particles"], 8192)
        self.assertIsNone(summary["bh2c_parallel_vs_serial_speedup"])

    def test_software_receipt_is_rejected(self):
        receipt = receipt_for(512)
        receipt["measurement_class"] = "software-validation"
        receipt["hardware_performance_claim_allowed"] = False
        with self.assertRaisesRegex(sweep.SweepError, "software-validation"):
            sweep.validate_receipt(
                receipt,
                particles=512,
                steps=3,
                oracle_limit=4096,
                benchmark_repeats=7,
            )

    def test_repeat_tree_mismatch_is_rejected(self):
        receipt = receipt_for(512)
        receipt["tree"]["repeat_rebuild_matches"] = False
        with self.assertRaisesRegex(sweep.SweepError, "repeat tree checksum"):
            sweep.validate_receipt(
                receipt,
                particles=512,
                steps=3,
                oracle_limit=4096,
                benchmark_repeats=7,
            )

    def test_bh2c_benchmark_must_skip_above_its_cap(self):
        receipt = receipt_for(8192)
        receipt["tree_build_benchmark"]["bh2c_serial"] = {
            "status": "executed",
            "parallel_vs_serial_speedup": 1.0,
        }
        with self.assertRaisesRegex(sweep.SweepError, "must skip above 4096"):
            sweep.validate_receipt(
                receipt,
                particles=8192,
                steps=3,
                oracle_limit=4096,
                benchmark_repeats=7,
            )


if __name__ == "__main__":
    unittest.main()

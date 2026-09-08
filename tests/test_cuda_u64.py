#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
import importlib.util
import math
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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

    def test_duplicate_json_keys_fail_before_validation(self):
        payloads = [
            '{"schema_version":1,"schema_version":1,"task":{}}',
            '{"schema_version":1,"task":{"steps":0,"steps":1}}',
            '{"schema_version":1,"task":{"physics":{"model":"nfw","model":"burkert"}}}',
        ]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "job.json"
            for payload in payloads:
                with self.subTest(payload=payload):
                    path.write_text(payload, encoding="utf-8")
                    with self.assertRaisesRegex(ValueError, "Duplicate JSON key"):
                        cuda.load_job(path)

    def test_unknown_fields_and_boolean_integers_fail_closed(self):
        with self.assertRaisesRegex(ValueError, "Unknown u64 task fields"):
            cuda.validate_job({"schema_version": 1, "task": {"particles": 4}})
        with self.assertRaisesRegex(ValueError, "direction must be an integer"):
            cuda.validate_job({"schema_version": 1, "task": {"direction": True}})

    def test_schema_version_requires_json_integer(self):
        for value in (True, 1.0, "1", None):
            with self.subTest(value=value):
                with self.assertRaisesRegex(ValueError, "schema_version must be an integer"):
                    cuda.validate_job({"schema_version": value, "task": {}})
        self.assertEqual(
            cuda.validate_job({"schema_version": 1, "task": {}})["schema_version"],
            1,
        )

    def test_validated_f64_fields_are_normalized_to_float(self):
        job = cuda.validate_job({
            "schema_version": 1,
            "task": {
                "physics": {
                    "disk_ml": 1,
                    "bulge_ml": 1,
                    "black_hole_million": 0,
                    "uff_v_inf": 120,
                    "uff_core": 3,
                    "uff_beta": 0,
                    "halo_log_mass": 11,
                    "halo_concentration": 10,
                    "burkert_log_density": 7,
                    "burkert_core": 5,
                    "mond_a0": 1,
                },
                "dt_myr": 1,
                "pitch_deg": 22,
                "scatter": 1,
                "bulge_fraction": 0,
                "thickness_kpc": 1,
                "radial_kick_kms": 0,
                "softening_kpc": 0.1,
                "extent_kpc": 16,
                "inclination_deg": 38,
            },
        })
        task = job["task"]
        for name in (
            "dt_myr", "pitch_deg", "scatter", "bulge_fraction", "thickness_kpc",
            "radial_kick_kms", "softening_kpc", "extent_kpc", "inclination_deg",
        ):
            with self.subTest(field=name):
                self.assertIs(type(task[name]), float)
        for name in (
            "disk_ml", "bulge_ml", "black_hole_million", "uff_v_inf", "uff_core",
            "uff_beta", "halo_log_mass", "halo_concentration", "burkert_log_density",
            "burkert_core", "mond_a0",
        ):
            with self.subTest(field=f"physics.{name}"):
                self.assertIs(type(task["physics"][name]), float)
        self.assertIn('"dt_myr": 1.0', __import__("json").dumps(job, indent=2))

    def test_snapshot_every_rejects_negative_and_out_of_u32_range(self):
        for value in (-1, 1 << 32):
            with self.subTest(value=value):
                with self.assertRaisesRegex(ValueError, "snapshot_every must be in 0..=2\^32-1"):
                    cuda.validate_job({
                        "schema_version": 1,
                        "task": {"snapshot_every": value},
                    })
        zero = cuda.validate_job({
            "schema_version": 1,
            "task": {"snapshot_every": 0, "steps": 3},
        })
        self.assertEqual(cuda.frame_steps(zero["task"]), [0, 3])

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

    def test_cuda_constants_are_generated_from_checked_in_demo_table(self):
        self.assertIn(f"#define GALAXY_DEMO_N {len(cuda.DEMO)}", cuda.CUDA_SOURCE)
        self.assertNotIn("__GALAXY_DEMO_CONSTANTS__", cuda.CUDA_SOURCE)
        for radius, gas, disk, bulge in cuda.DEMO:
            self.assertIn(cuda._cuda_f32_literal(radius), cuda.CUDA_SOURCE)
            self.assertIn(cuda._cuda_f32_literal(gas), cuda.CUDA_SOURCE)
            self.assertIn(cuda._cuda_f32_literal(disk), cuda.CUDA_SOURCE)
            self.assertIn(cuda._cuda_f32_literal(bulge), cuda.CUDA_SOURCE)

    def test_kernel_schedule_matches_integrator(self):
        circular = cuda.validate_job({"schema_version": 1, "task": {}})["task"]
        leapfrog = cuda.validate_job({
            "schema_version": 1,
            "task": {"integrator": "leapfrog"},
        })["task"]
        self.assertIn("circular phase kernel", cuda.cuda_kernel_schedule(circular))
        self.assertNotIn("leapfrog", cuda.cuda_kernel_schedule(circular))
        self.assertIn("bounded CUDA launches", cuda.cuda_kernel_schedule(leapfrog))
        self.assertIn("particle-updates per launch", cuda.cuda_kernel_schedule(leapfrog))

    def test_long_leapfrog_intervals_are_chunked_without_intermediate_sampling(self):
        self.assertEqual(cuda.leapfrog_steps_per_launch(cuda.MAX_TILE_PARTICLES), 1)
        self.assertLessEqual(
            cuda.leapfrog_steps_per_launch(4096),
            cuda.MAX_LEAPFROG_STEPS_PER_LAUNCH,
        )

        class FakeNp:
            @staticmethod
            def uint32(value):
                return int(value)

            @staticmethod
            def float32(value):
                return float(value)

        gpu = object.__new__(cuda.CudaGpu)
        gpu.np = FakeNp()
        gpu.k_leapfrog = object()
        gpu._physics_args = lambda task: ()
        launches = []
        gpu._launch = lambda kernel, count, args: launches.append(int(args[2]))
        gpu._timed = lambda fn: (fn(), 0.5)[1]

        task = cuda.validate_job({
            "schema_version": 1,
            "task": {
                "integrator": "leapfrog",
                "steps": 100_000,
                "snapshot_every": 0,
            },
        })["task"]
        elapsed = gpu.advance(
            {"particles": object(), "count": 4096},
            task,
            100_000,
            100_000,
        )
        self.assertEqual(elapsed, 0.5)
        self.assertEqual(sum(launches), 100_000)
        self.assertGreater(len(launches), 1)
        self.assertLessEqual(max(launches), cuda.MAX_LEAPFROG_STEPS_PER_LAUNCH)
        self.assertTrue(all(chunk * 4096 <= cuda.MAX_LEAPFROG_PARTICLE_UPDATES_PER_LAUNCH for chunk in launches))

    def test_force_model_description_names_selected_physics(self):
        for model in cuda.MODEL_IDS:
            with self.subTest(model=model, integrator="circular"):
                task = cuda.validate_job({
                    "schema_version": 1,
                    "task": {"physics": {"model": model}},
                })["task"]
                self.assertIn(model, cuda.force_model_description(task))
                self.assertIn("circular speed", cuda.force_model_description(task))
            with self.subTest(model=model, integrator="leapfrog"):
                task = cuda.validate_job({
                    "schema_version": 1,
                    "task": {
                        "physics": {"model": model},
                        "integrator": "leapfrog",
                    },
                })["task"]
                self.assertIn(model, cuda.force_model_description(task))
                self.assertIn("radial acceleration", cuda.force_model_description(task))

    def test_sample_cache_uses_packed_numpy_storage(self):
        source = cuda.IMPLEMENTATION_PATH.read_text(encoding="utf-8")
        self.assertNotIn("host.tolist()", source)
        self.assertIn("np.empty((len(steps), samples, 8), dtype=np.float32)", source)
        self.assertIn("np.zeros((len(steps), samples), dtype=np.bool_)", source)
        self.assertIn('"sample_cache_host_representation": "packed NumPy float32 array plus boolean validity bitmap"', source)

    def test_requested_backend_provenance_preserves_auto(self):
        with mock.patch.dict(os.environ, {"GALAXY_BACKEND_REQUESTED": "auto"}, clear=False):
            self.assertEqual(cuda.requested_backend(), "auto")
        with mock.patch.dict(os.environ, {"GALAXY_BACKEND_REQUESTED": "cuda"}, clear=False):
            self.assertEqual(cuda.requested_backend(), "cuda")
        with mock.patch.dict(os.environ, {"GALAXY_BACKEND_REQUESTED": "vulkan"}, clear=False):
            with self.assertRaisesRegex(RuntimeError, "invalid requested backend provenance"):
                cuda.requested_backend()

    def test_runtime_source_hash_covers_entrypoint_implementation_and_router(self):
        self.assertIn(MODULE_PATH.resolve(), cuda.RUNTIME_SOURCE_PATHS)
        self.assertIn(cuda.IMPLEMENTATION_PATH.resolve(), cuda.RUNTIME_SOURCE_PATHS)
        self.assertIn(cuda.BOOTSTRAP_PATH, cuda.RUNTIME_SOURCE_PATHS)
        self.assertIn(cuda.RUNNER_PATH, cuda.RUNTIME_SOURCE_PATHS)
        digest = cuda.runtime_source_sha256()
        self.assertEqual(len(digest), 64)
        int(digest, 16)

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

#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
import importlib.util
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "scripts" / "bench-bh2d-hardware.py"
SPEC = importlib.util.spec_from_file_location("bench_bh2d_hardware", MODULE_PATH)
sweep = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(sweep)


STATE_ERROR = {
    "position_rms_relative_l2": 0.001,
    "position_max_relative": 0.01,
    "velocity_rms_relative_l2": 0.001,
    "velocity_max_relative": 0.01,
}


def receipt_for(
    particles: int,
    *,
    preset: str = "disc",
    steps: int = 3,
    dt_myr: float = 0.01,
    seed: int = 303,
    theta: float = 0.5,
    softening_kpc: float = 0.05,
    direct_probes: int = 12,
    oracle_limit: int = 4096,
    warmup: int = 2,
    repeats: int = 7,
):
    # Synthetic tree statistics scale with the workload so the shared fixture
    # remains structurally valid as validator invariants tighten. These are
    # test-only values, not measured GPU topology evidence.
    leaf_count = particles
    active_cell_count = 2 * leaf_count - 1
    max_depth = (particles - 1).bit_length()

    force_rms = 0.001
    force_max = force_rms * min(particles ** 0.5, 10.0)
    probe_count = min(direct_probes, particles)
    direct_rms = 0.001
    direct_max = direct_rms * (probe_count ** 0.5)

    oracle_status = "executed" if particles <= oracle_limit else "skipped-particle-limit"
    if oracle_status == "executed":
        oracles = {
            "status": "executed",
            "bh2c_serial_gpu": {
                "state_error": dict(STATE_ERROR),
                "force_rms_relative": force_rms,
                "force_max_relative": force_max,
            },
            "bh2b2_host_tree_gpu": {
                "state_error": dict(STATE_ERROR),
                "force_rms_relative": force_rms,
                "force_max_relative": force_max,
            },
        }
        trajectory = {"status": "executed", "state_error": dict(STATE_ERROR)}
        flat_rms = force_rms
        flat_max = force_max
    else:
        oracles = {
            "status": "skipped-particle-limit",
            "limit": oracle_limit,
            "reason": "bounded oracle",
        }
        trajectory = {
            "status": "skipped-particle-limit",
            "limit": oracle_limit,
            "reason": "bounded trajectory",
        }
        flat_rms = None
        flat_max = None

    parallel_samples = [0.005] * repeats
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
        "preset": preset,
        "particles": particles,
        "steps": steps,
        "dt_myr": dt_myr,
        "simulated_time_myr": dt_myr * steps,
        "seed": seed,
        "theta": theta,
        "softening_kpc": softening_kpc,
        "gpu": {
            "index": 0,
            "name": "Synthetic GPU",
            "backend": "Vulkan",
            "device_type": "DiscreteGpu",
            "driver": "synthetic-driver",
            "driver_info": "1.0",
            "software": False,
        },
        "host_tree_rebuilds": 0,
        "host_particle_readbacks_during_steps": 0,
        "force_solves": steps + 1,
        "evolution_tree_builds": steps + 1,
        "parallel_tree_buffer_bytes": sweep.expected_parallel_tree_buffer_bytes(particles),
        "tree": {
            "initial_checksum_fnv_mix64": "1111111111111111",
            "final_checksum_fnv_mix64": "2222222222222222",
            "repeat_checksum_fnv_mix64": "2222222222222222",
            "repeat_rebuild_matches": True,
            "active_cell_count": active_cell_count,
            "leaf_count": leaf_count,
            "max_depth": max_depth,
        },
        "final_force": {
            "bh2a_flat_status": oracle_status,
            "gpu_vs_bh2a_flat_rms_relative": flat_rms,
            "gpu_vs_bh2a_flat_max_relative": flat_max,
            "direct_probe_count": probe_count,
            "direct_probe_rms_relative": direct_rms,
            "direct_probe_max_relative": direct_max,
        },
        "trajectory_vs_bh2a_flat_f64": trajectory,
        "gpu_oracles": oracles,
        "tree_build_benchmark": {
            "sample_scope": sweep.BENCHMARK_SAMPLE_SCOPE,
            "stage_timings_are_diagnostics": True,
            "warmup": warmup,
            "repeats": repeats,
            "parallel_samples_seconds": parallel_samples,
            "parallel_median_seconds": 0.005,
            "bh2c_serial": serial,
        },
    }


def validation_kwargs(particles: int, **overrides):
    values = {
        "particles": particles,
        "preset": "disc",
        "steps": 3,
        "dt_myr": 0.01,
        "seed": 303,
        "theta": 0.5,
        "softening_kpc": 0.05,
        "direct_probes": 12,
        "oracle_limit": 4096,
        "benchmark_warmup": 2,
        "benchmark_repeats": 7,
        "adapter_selector": None,
    }
    values.update(overrides)
    return values


class Bh2dHardwareSweepTests(unittest.TestCase):
    def test_particle_list_must_be_strictly_increasing(self):
        self.assertEqual(sweep.parse_particles("512,4096,8192"), [512, 4096, 8192])
        for invalid in ("", "4096,512", "512,512", "1", "65537", "512,nope"):
            with self.subTest(invalid=invalid):
                with self.assertRaises(sweep.SweepError):
                    sweep.parse_particles(invalid)

    def test_cleanliness_rejects_untracked_cargo_config(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.name", "GALAXY Test"], cwd=repo, check=True)
            subprocess.run(
                ["git", "config", "user.email", "galaxy-test@example.invalid"],
                cwd=repo,
                check=True,
            )
            (repo / "tracked.txt").write_text("tracked\n")
            subprocess.run(["git", "add", "tracked.txt"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "fixture"], cwd=repo, check=True)

            cargo = repo / ".cargo"
            cargo.mkdir()
            (cargo / "config.toml").write_text('[build]\nrustflags = ["-C", "target-cpu=native"]\n')

            with self.assertRaisesRegex(sweep.SweepError, "untracked files"):
                sweep.require_clean_source_tree(repo)

    def test_ignored_cargo_config_is_rejected_even_when_git_hides_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.name", "GALAXY Test"], cwd=repo, check=True)
            subprocess.run(
                ["git", "config", "user.email", "galaxy-test@example.invalid"],
                cwd=repo,
                check=True,
            )
            (repo / "tracked.txt").write_text("tracked\n")
            subprocess.run(["git", "add", "tracked.txt"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "fixture"], cwd=repo, check=True)

            info_exclude = repo / ".git" / "info" / "exclude"
            info_exclude.write_text(".cargo/\n")
            cargo = repo / ".cargo"
            cargo.mkdir()
            (cargo / "config.toml").write_text('[build]\nrustflags = ["-C", "target-cpu=native"]\n')

            sweep.require_clean_source_tree(repo)
            with self.assertRaisesRegex(sweep.SweepError, "Cargo configuration"):
                sweep.cargo_config_context(repo)

    def test_cleanliness_rejects_assume_unchanged_and_skip_worktree(self):
        for flag in ("--assume-unchanged", "--skip-worktree"):
            with self.subTest(flag=flag), tempfile.TemporaryDirectory() as tmp:
                repo = Path(tmp)
                subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
                subprocess.run(["git", "config", "user.name", "GALAXY Test"], cwd=repo, check=True)
                subprocess.run(
                    ["git", "config", "user.email", "galaxy-test@example.invalid"],
                    cwd=repo,
                    check=True,
                )
                path = repo / "tracked.txt"
                path.write_text("one\n")
                subprocess.run(["git", "add", "tracked.txt"], cwd=repo, check=True)
                subprocess.run(["git", "commit", "-qm", "fixture"], cwd=repo, check=True)
                subprocess.run(["git", "update-index", flag, "tracked.txt"], cwd=repo, check=True)
                path.write_text("two\n")

                self.assertEqual(
                    subprocess.run(
                        ["git", "diff", "--quiet", "--"],
                        cwd=repo,
                        check=False,
                    ).returncode,
                    0,
                )
                with self.assertRaisesRegex(sweep.SweepError, "index flags"):
                    sweep.require_clean_source_tree(repo)

    def test_cleanliness_uses_raw_bytes_instead_of_clean_filters(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = root / "repo"
            repo.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.name", "GALAXY Test"], cwd=repo, check=True)
            subprocess.run(
                ["git", "config", "user.email", "galaxy-test@example.invalid"],
                cwd=repo,
                check=True,
            )
            source = repo / "source.rs"
            source.write_text("original\n")
            subprocess.run(["git", "add", "source.rs"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "fixture"], cwd=repo, check=True)

            clean = root / "clean-filter.sh"
            clean.write_text("#!/bin/sh\nsed 's/replacement/original/g'\n")
            clean.chmod(0o755)
            subprocess.run(
                ["git", "config", "filter.hide.clean", str(clean)],
                cwd=repo,
                check=True,
            )
            info_attributes = repo / ".git" / "info" / "attributes"
            info_attributes.write_text("source.rs filter=hide\n")
            source.write_text("replacement\n")

            self.assertEqual(
                subprocess.run(
                    ["git", "diff", "--quiet", "--", "source.rs"],
                    cwd=repo,
                    check=False,
                ).returncode,
                0,
            )
            with self.assertRaisesRegex(sweep.SweepError, "raw bytes"):
                sweep.require_clean_source_tree(repo)

    def test_cleanliness_allows_only_the_evidence_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.name", "GALAXY Test"], cwd=repo, check=True)
            subprocess.run(
                ["git", "config", "user.email", "galaxy-test@example.invalid"],
                cwd=repo,
                check=True,
            )
            (repo / "tracked.txt").write_text("tracked\n")
            subprocess.run(["git", "add", "tracked.txt"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "fixture"], cwd=repo, check=True)

            output = repo / "runs" / "evidence"
            output.mkdir(parents=True)
            (output / "manifest.json").write_text("{}\n")
            sweep.require_clean_source_tree(repo, output)

            (repo / "unexpected.txt").write_text("changes build provenance\n")
            with self.assertRaisesRegex(sweep.SweepError, "unexpected.txt"):
                sweep.require_clean_source_tree(repo, output)

    def test_source_provenance_rejects_head_change(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.name", "GALAXY Test"], cwd=repo, check=True)
            subprocess.run(
                ["git", "config", "user.email", "galaxy-test@example.invalid"],
                cwd=repo,
                check=True,
            )
            path = repo / "tracked.txt"
            path.write_text("one\n")
            subprocess.run(["git", "add", "tracked.txt"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "one"], cwd=repo, check=True)
            revision = sweep.git_revision(repo)

            path.write_text("two\n")
            subprocess.run(["git", "add", "tracked.txt"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "two"], cwd=repo, check=True)

            with self.assertRaisesRegex(sweep.SweepError, "source revision changed"):
                sweep.require_source_provenance(repo, revision)

    def test_git_replacement_objects_cannot_rewrite_provenance(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.name", "GALAXY Test"], cwd=repo, check=True)
            subprocess.run(
                ["git", "config", "user.email", "galaxy-test@example.invalid"],
                cwd=repo,
                check=True,
            )
            source = repo / "source.rs"
            source.write_text("original\n")
            subprocess.run(["git", "add", "source.rs"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "A"], cwd=repo, check=True)
            revision_a = subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=repo, text=True
            ).strip()

            source.write_text("replacement\n")
            subprocess.run(["git", "add", "source.rs"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "B"], cwd=repo, check=True)
            revision_b = subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=repo, text=True
            ).strip()

            subprocess.run(["git", "checkout", "--detach", "-q", revision_a], cwd=repo, check=True)
            subprocess.run(["git", "replace", revision_a, revision_b], cwd=repo, check=True)
            subprocess.run(["git", "reset", "--hard", "-q", "HEAD"], cwd=repo, check=True)
            self.assertEqual(source.read_text(), "replacement\n")
            self.assertEqual(sweep.git_revision(repo), revision_a)
            with self.assertRaisesRegex(sweep.SweepError, "differs from HEAD|raw bytes"):
                sweep.require_clean_source_tree(repo)

    def test_cargo_config_context_detects_mid_sweep_change(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "config", "user.name", "GALAXY Test"], cwd=repo, check=True)
            subprocess.run(
                ["git", "config", "user.email", "galaxy-test@example.invalid"],
                cwd=repo,
                check=True,
            )
            cargo = repo / ".cargo"
            cargo.mkdir()
            config = cargo / "config.toml"
            config.write_text("[build]\nincremental = false\n")
            subprocess.run(["git", "add", ".cargo/config.toml"], cwd=repo, check=True)
            subprocess.run(["git", "commit", "-qm", "fixture"], cwd=repo, check=True)
            revision = sweep.git_revision(repo)
            context = sweep.cargo_config_context(repo)

            config.write_text("[build]\nincremental = true\n")
            with self.assertRaises(sweep.SweepError):
                sweep.require_source_provenance(
                    repo,
                    revision,
                    expected_cargo_context=context,
                )

    def test_build_environment_overrides_are_rejected(self):
        for name in (
            "LD_AUDIT",
            "RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
            "RUSTC",
            "RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
            "RUSTUP_TOOLCHAIN",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER",
            "CARGO_PROFILE_RELEASE_LTO",
            "CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS",
            "CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS",
            "CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_OPT_LEVEL",
        ):
            with self.subTest(name=name), mock.patch.dict(
                os.environ,
                {name: "evidence-changing-value"},
                clear=False,
            ):
                with self.assertRaisesRegex(sweep.SweepError, name):
                    sweep.require_no_build_environment_overrides()

    def test_dependency_source_context_binds_cached_source_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            repo = root / "repo"
            repo.mkdir()
            runtime = repo / "runtime"
            runtime.mkdir()
            (runtime / "Cargo.toml").write_text("[package]\nname='fixture'\nversion='0.1.0'\n")
            dependency = root / "cargo-home" / "registry" / "src" / "index" / "wgpu-24.0.5"
            dependency.mkdir(parents=True)
            manifest = dependency / "Cargo.toml"
            source = dependency / "src" / "lib.rs"
            source.parent.mkdir()
            manifest.write_text("[package]\nname='wgpu'\nversion='24.0.5'\n")
            source.write_text("pub const VALUE: u32 = 1;\n")
            metadata = {
                "packages": [
                    {
                        "name": "fixture",
                        "version": "0.1.0",
                        "source": None,
                        "manifest_path": str(runtime / "Cargo.toml"),
                    },
                    {
                        "name": "wgpu",
                        "version": "24.0.5",
                        "source": "registry+https://github.com/rust-lang/crates.io-index",
                        "manifest_path": str(manifest),
                    },
                ]
            }
            completed = subprocess.CompletedProcess(
                [],
                0,
                stdout=__import__("json").dumps(metadata).encode(),
                stderr=b"",
            )
            toolchain = {
                "cargo": {"executable": "/selected/cargo"},
                "rustc": {"executable": "/selected/rustc"},
            }
            with mock.patch.object(sweep.subprocess, "run", return_value=completed):
                first = sweep.dependency_source_context(repo, toolchain)
                source.write_text("pub const VALUE: u32 = 2;\n")
                second = sweep.dependency_source_context(repo, toolchain)
            self.assertEqual(len(first), 1)
            self.assertEqual(first[0]["name"], "wgpu")
            self.assertNotEqual(first[0]["tree_sha256"], second[0]["tree_sha256"])

    def test_cargo_config_execution_and_source_redirects_are_rejected(self):
        cases = {
            "rustc-wrapper": '[build]\nrustc-wrapper = "/tmp/wrapper"\n',
            "target-linker": (
                '[target.x86_64-unknown-linux-gnu]\n'
                'linker = "/tmp/linker"\n'
            ),
            "target-runner": (
                '[target.x86_64-unknown-linux-gnu]\n'
                'runner = "/tmp/runner"\n'
            ),
            "target-rustflags": (
                '[target.x86_64-unknown-linux-gnu]\n'
                'rustflags = ["-C", "linker=/tmp/linker"]\n'
            ),
            "source": (
                '[source.crates-io]\n'
                'replace-with = "vendored"\n'
                '[source.vendored]\n'
                'directory = "/tmp/vendor"\n'
            ),
            "paths": 'paths = ["/tmp/override"]\n',
            "env": '[env]\nRUSTFLAGS = "-C target-cpu=native"\n',
        }
        for name, payload in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                repo = Path(tmp)
                subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
                subprocess.run(["git", "config", "user.name", "GALAXY Test"], cwd=repo, check=True)
                subprocess.run(
                    ["git", "config", "user.email", "galaxy-test@example.invalid"],
                    cwd=repo,
                    check=True,
                )
                cargo = repo / ".cargo"
                cargo.mkdir()
                config = cargo / "config.toml"
                config.write_text(payload)
                subprocess.run(["git", "add", ".cargo/config.toml"], cwd=repo, check=True)
                subprocess.run(["git", "commit", "-qm", "fixture"], cwd=repo, check=True)

                with self.assertRaisesRegex(sweep.SweepError, "unsupported|not allowed"):
                    sweep.cargo_config_context(repo)

    def test_toolchain_context_records_resolved_binaries_and_versions(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            cargo = bin_dir / "cargo-custom"
            rustc = bin_dir / "rustc"
            cargo.write_text("#!/bin/sh\necho 'cargo 9.9.9 (fixture)'\n")
            rustc.write_text("#!/bin/sh\necho 'rustc 9.9.9 (fixture)'\n")
            cargo.chmod(0o755)
            rustc.chmod(0o755)

            with mock.patch.dict(
                os.environ,
                {"PATH": str(bin_dir)},
                clear=True,
            ):
                context = sweep.toolchain_context("cargo-custom", root)

            self.assertEqual(context["cargo"]["requested"], "cargo-custom")
            self.assertEqual(context["cargo"]["version_verbose"], "cargo 9.9.9 (fixture)")
            self.assertEqual(context["rustc"]["version_verbose"], "rustc 9.9.9 (fixture)")
            self.assertEqual(len(context["cargo"]["sha256"]), 64)
            self.assertEqual(len(context["rustc"]["sha256"]), 64)

    def test_cli_requires_isolation_before_optional_imports(self):
        import sys
        for isolated, expected in ((False, 1), (True, 0)):
            result = subprocess.run(
                [sys.executable, *(["-I"] if isolated else []), str(MODULE_PATH),
                 "--output", "/unused", "--particles", "512", "--dry-run"],
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, expected, result.stderr)
            if not isolated:
                self.assertIn("isolated Python", result.stderr)

    def test_system_build_path_and_explicit_rustc(self):
        with mock.patch.dict(
            os.environ,
            {
                "PATH": "/unrecorded/bin",
                "GIT_WORK_TREE": "/other",
                "LD_AUDIT": "/tmp/audit.so",
                "LD_PRELOAD": "/tmp/preload.so",
            },
        ):
            env = sweep.build_environment({"rustc": {"executable": "/selected/bin/rustc"}})
        self.assertEqual(env["PATH"], "/usr/bin:/bin")
        self.assertEqual(env["RUSTC"], "/selected/bin/rustc")
        self.assertNotIn("GIT_WORK_TREE", env)
        self.assertNotIn("LD_AUDIT", env)
        self.assertNotIn("LD_PRELOAD", env)

    def test_git_invocation_binds_checkout_and_clears_selectors(self):
        with mock.patch.dict(os.environ, {"GIT_WORK_TREE": "/other", "GIT_DIR": "/other/.git",
                                          "GIT_INDEX_FILE": "/other/index"}):
            command, env = sweep.git_invocation(["git", "status"], ROOT)
        self.assertIn(str(ROOT), command)
        self.assertIn(str(ROOT / ".git"), command)
        for name in ("GIT_WORK_TREE", "GIT_DIR", "GIT_INDEX_FILE"):
            self.assertNotIn(name, env)
        self.assertEqual(sweep.git_revision(ROOT),
                         subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip())

    def test_relative_cargo_home_is_relative_to_repo(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            home = root / "cargo-home"
            home.mkdir()
            config = home / "config.toml"
            config.write_text("[build]\nincremental = false\n")
            with mock.patch.dict(os.environ, {"CARGO_HOME": "cargo-home"}):
                self.assertIn(config, sweep.effective_cargo_config_paths(root))

    def test_rustup_proxy_records_selected_binary(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            rustup = root / "rustup"
            rustup.write_bytes(b"proxy identity")
            proxy = root / "cargo"
            proxy.symlink_to(rustup)
            selected = root / "selected-cargo"
            selected.write_bytes(b"selected compiler tool")
            with mock.patch.object(sweep, "resolve_executable", return_value=proxy), \
                 mock.patch.object(sweep, "run_checked", side_effect=[str(selected), "cargo fixture"]):
                record = sweep.tool_record("cargo", "cargo", root)
            self.assertEqual(record["executable"], str(selected))
            self.assertEqual(record["sha256"], sweep.sha256_file(selected))
            self.assertEqual(record["proxy_sha256"], sweep.sha256_file(rustup))
            self.assertNotEqual(record["sha256"], record["proxy_sha256"])

    def test_hardlinked_rustup_proxy_uses_rustup_command_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            rustup = root / "rustup"
            rustup.write_bytes(b"proxy")
            proxy = root / "cargo"
            os.link(rustup, proxy)
            selected = root / "toolchain-cargo"
            selected.write_bytes(b"cargo")
            with mock.patch.object(sweep, "resolve_executable", return_value=proxy), \
                 mock.patch.object(sweep.shutil, "which", return_value=str(rustup)), \
                 mock.patch.object(sweep, "run_checked", side_effect=[str(selected), "cargo fixture"]) as run:
                record = sweep.tool_record("cargo", "cargo", root)
            self.assertEqual(run.call_args_list[0].args[0], [str(rustup), "which", "cargo"])
            self.assertEqual(record["sha256"], sweep.sha256_file(selected))

    def test_invalid_workloads_do_not_create_output(self):
        import sys
        for flag, value in (("--dt-myr", "nan"), ("--theta", "inf"),
                            ("--softening-kpc", "-1"), ("--direct-probes", "65"),
                            ("--seed", str(2**64))):
            with self.subTest(flag=flag), tempfile.TemporaryDirectory() as tmp:
                output = Path(tmp) / "evidence"
                result = subprocess.run([sys.executable, "-I", str(MODULE_PATH),
                                         "--output", str(output), flag, value], capture_output=True)
                self.assertEqual(result.returncode, 1)
                self.assertFalse(output.exists())

    def test_integer_parse_limit_is_normalized(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "receipt.json"
            path.write_text('{"value":' + '1' * 5000 + '}')
            with self.assertRaisesRegex(sweep.SweepError, "not valid JSON"):
                sweep.load_receipt(path)

    def test_non_utf8_run_log_preserved_and_manifest_failed(self):
        import argparse
        import json
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "evidence"
            args = argparse.Namespace(output=output, particles="512", preset="disc", steps=3,
                                      dt_myr=0.01, seed=303, theta=0.5, softening_kpc=0.05,
                                      direct_probes=12, oracle_limit=4096, benchmark_warmup=2,
                                      benchmark_repeats=7, adapter=None, cargo="cargo", dry_run=False)
            context = {"cargo": {"executable": "/selected/cargo"},
                       "rustc": {"executable": "/selected/rustc"}}
            with mock.patch.object(sweep, "parse_args", return_value=args), \
                 mock.patch.object(sweep, "git_revision", return_value="fixture"), \
                 mock.patch.object(sweep, "require_clean_source_tree"), \
                 mock.patch.object(sweep, "cargo_config_context", return_value=[]), \
                 mock.patch.object(sweep, "toolchain_context", return_value=context), \
                 mock.patch.object(sweep, "dependency_source_context", return_value=[]), \
                 mock.patch.object(sweep, "require_source_provenance"), \
                 mock.patch.object(sweep.platform, "platform", return_value="fixture"), \
                 mock.patch.object(sweep.subprocess, "run", return_value=                     subprocess.CompletedProcess([], 1, b"driver: \xff\n")):
                self.assertEqual(sweep.main(), 1)
            self.assertEqual((output / "n000512/run.log").read_bytes(), b"driver: \xff\n")
            self.assertEqual(json.loads((output / "manifest.json").read_text())["status"], "failed")

    def test_invalid_utf8_receipt_is_normalized_to_sweep_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "receipt.json"
            path.write_bytes(b"{\xff}")
            with self.assertRaisesRegex(sweep.SweepError, "not valid UTF-8"):
                sweep.load_receipt(path)

    def test_verifier_command_uses_fresh_explicit_target_directory(self):
        args = __import__("argparse").Namespace(
            cargo="cargo",
            preset="disc",
            steps=3,
            dt_myr=0.01,
            seed=303,
            theta=0.5,
            softening_kpc=0.05,
            direct_probes=12,
            oracle_limit=4096,
            benchmark_warmup=2,
            benchmark_repeats=7,
            adapter=None,
        )
        receipt = Path("/evidence/n000512/receipt.json")
        target_dir = Path("/evidence/n000512/cargo-target")
        command = sweep.command_for(args, 512, receipt, target_dir)
        self.assertIn("--target-dir", command)
        index = command.index("--target-dir")
        self.assertEqual(command[index + 1], str(target_dir))
        self.assertNotIn("runtime/target", " ".join(command))

    def test_collision_dry_run_rejects_fewer_than_four_particles(self):
        import sys
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(
                [
                    sys.executable,
                    "-I",
                    str(MODULE_PATH),
                    "--output",
                    str(Path(tmp) / "plan"),
                    "--preset",
                    "collision",
                    "--particles",
                    "2",
                    "--dry-run",
                ],
                capture_output=True,
                text=True,
            )
        self.assertEqual(result.returncode, 1)
        self.assertIn("at least 4", result.stderr)

    def test_dry_run_does_not_execute_cargo(self):
        import sys
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            marker = root / "executed"
            fake_cargo = root / "cargo"
            fake_cargo.write_text(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> " + repr(str(marker)) + "\n"
            )
            fake_cargo.chmod(0o755)
            result = subprocess.run(
                [
                    sys.executable,
                    "-I",
                    str(MODULE_PATH),
                    "--output",
                    str(root / "plan"),
                    "--particles",
                    "512",
                    "--cargo",
                    str(fake_cargo),
                    "--dry-run",
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(marker.exists())
            self.assertTrue(result.stdout.startswith(str(fake_cargo)))

    def test_dry_run_normalizes_output_and_cargo_paths(self):
        import sys
        result = subprocess.run(
            [
                sys.executable,
                "-I",
                str(MODULE_PATH),
                "--output",
                "~/evidence",
                "--particles",
                "512",
                "--dry-run",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        command = __import__("shlex").split(result.stdout.strip())
        self.assertTrue(Path(command[0]).is_absolute())
        expected = str((Path.home() / "evidence" / "n000512" / "receipt.json").resolve())
        self.assertIn(expected, command)
        self.assertNotIn("~/evidence", result.stdout)

    def test_shared_receipt_fixture_is_valid_across_sweep_sizes(self):
        for particles in (512, 4096, 8192, 65536):
            with self.subTest(particles=particles):
                summary = sweep.validate_receipt(
                    receipt_for(particles),
                    **validation_kwargs(particles),
                )
                self.assertEqual(summary["particles"], particles)

    def test_hardware_receipt_at_oracle_size_is_accepted(self):
        summary = sweep.validate_receipt(receipt_for(4096), **validation_kwargs(4096))
        self.assertEqual(summary["particles"], 4096)
        self.assertEqual(summary["bh2c_parallel_vs_serial_speedup"], 2.0)
        self.assertEqual(summary["parallel_median_seconds"], 0.005)

    def test_large_hardware_receipt_requires_explicit_oracle_skips(self):
        summary = sweep.validate_receipt(receipt_for(8192), **validation_kwargs(8192))
        self.assertEqual(summary["particles"], 8192)
        self.assertIsNone(summary["bh2c_parallel_vs_serial_speedup"])

    def test_receipt_must_bind_the_full_requested_workload(self):
        mutations = {
            "preset": "collision",
            "dt_myr": 1.0,
            "seed": 999,
            "theta": 2.0,
            "softening_kpc": 100.0,
        }
        for field, value in mutations.items():
            with self.subTest(field=field):
                receipt = receipt_for(512)
                receipt[field] = value
                with self.assertRaisesRegex(sweep.SweepError, "requested workload"):
                    sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["final_force"]["direct_probe_count"] = 1
        with self.assertRaisesRegex(sweep.SweepError, "direct-probe count"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree_build_benchmark"]["warmup"] = 9
        with self.assertRaisesRegex(sweep.SweepError, "warmup count"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree_build_benchmark"]["repeats"] = 6
        with self.assertRaisesRegex(sweep.SweepError, "repeat count"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_adapter_index_is_bound_into_identity_and_numeric_selector(self):
        receipt0 = receipt_for(512)
        receipt1 = receipt_for(512)
        receipt1["gpu"]["index"] = 1
        summary0 = sweep.validate_receipt(receipt0, **validation_kwargs(512))
        summary1 = sweep.validate_receipt(receipt1, **validation_kwargs(512))
        self.assertNotEqual(summary0["adapter_identity"], summary1["adapter_identity"])

        with self.assertRaisesRegex(sweep.SweepError, "numeric adapter selector"):
            sweep.validate_receipt(
                receipt1,
                **validation_kwargs(512, adapter_selector="0"),
            )

    def test_adapter_identity_is_mandatory_and_hardware_bound(self):
        for gpu in (
            {},
            {
                "name": "Synthetic GPU",
                "backend": "Vulkan",
                "device_type": "DiscreteGpu",
                "driver": "",
                "driver_info": "",
                "software": False,
            },
        ):
            with self.subTest(gpu=gpu):
                receipt = receipt_for(512)
                receipt["gpu"] = gpu
                with self.assertRaises(sweep.SweepError):
                    sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_software_receipt_is_rejected(self):
        receipt = receipt_for(512)
        receipt["measurement_class"] = "software-validation"
        receipt["hardware_performance_claim_allowed"] = False
        receipt["gpu"]["software"] = True
        with self.assertRaisesRegex(sweep.SweepError, "software-validation"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_malformed_typed_receipt_is_rejected_as_sweep_error(self):
        receipt = receipt_for(512)
        receipt["tree"]["active_cell_count"] = "corrupt"
        with self.assertRaisesRegex(sweep.SweepError, "active_cell_count must be an integer"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_repeat_tree_mismatch_is_rejected(self):
        receipt = receipt_for(512)
        receipt["tree"]["repeat_rebuild_matches"] = False
        with self.assertRaisesRegex(sweep.SweepError, "repeat tree checksum"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_repeat_tree_checksum_fields_are_required_and_compared(self):
        receipt = receipt_for(512)
        del receipt["tree"]["final_checksum_fnv_mix64"]
        with self.assertRaisesRegex(sweep.SweepError, "final_checksum_fnv_mix64"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["repeat_checksum_fnv_mix64"] = "3333333333333333"
        with self.assertRaisesRegex(sweep.SweepError, "does not match"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["repeat_checksum_fnv_mix64"] = "NOT-A-CHECKSUM"
        with self.assertRaisesRegex(sweep.SweepError, "16-digit lowercase hexadecimal"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_tree_structure_is_bounded_by_frozen_sparse_representation(self):
        receipt = receipt_for(512)
        receipt["tree"]["active_cell_count"] = sweep.TREE_LEVELS * 512 + 1
        with self.assertRaisesRegex(sweep.SweepError, "sparse-cell capacity"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["active_cell_count"] = 17
        receipt["tree"]["leaf_count"] = 18
        with self.assertRaisesRegex(sweep.SweepError, "cannot exceed active_cell_count"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["active_cell_count"] = 1201
        receipt["tree"]["leaf_count"] = 600
        receipt["tree"]["max_depth"] = 16
        with self.assertRaisesRegex(sweep.SweepError, "resident particle count"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["max_depth"] = 17
        with self.assertRaisesRegex(sweep.SweepError, "frozen Morton depth"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["active_cell_count"] = 1
        receipt["tree"]["leaf_count"] = 1
        receipt["tree"]["max_depth"] = 0
        with self.assertRaisesRegex(sweep.SweepError, "bucket size"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["active_cell_count"] = 2
        receipt["tree"]["leaf_count"] = 1
        receipt["tree"]["max_depth"] = 16
        with self.assertRaisesRegex(sweep.SweepError, "too small"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["active_cell_count"] = 2
        receipt["tree"]["leaf_count"] = 1
        receipt["tree"]["max_depth"] = 1
        with self.assertRaisesRegex(sweep.SweepError, "bucket size"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["active_cell_count"] = 145
        receipt["tree"]["leaf_count"] = 129
        receipt["tree"]["max_depth"] = 16
        with self.assertRaisesRegex(sweep.SweepError, "fan-out"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["tree"]["active_cell_count"] = 17
        receipt["tree"]["leaf_count"] = 12
        receipt["tree"]["max_depth"] = 16
        with self.assertRaisesRegex(sweep.SweepError, "internal cells"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_force_rms_cannot_exceed_reported_maximum(self):
        receipt = receipt_for(512)
        receipt["final_force"]["direct_probe_rms_relative"] = 0.03
        receipt["final_force"]["direct_probe_max_relative"] = 0.001
        with self.assertRaisesRegex(sweep.SweepError, "direct-force RMS cannot exceed maximum"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["final_force"]["gpu_vs_bh2a_flat_rms_relative"] = 0.03
        receipt["final_force"]["gpu_vs_bh2a_flat_max_relative"] = 0.001
        with self.assertRaisesRegex(sweep.SweepError, "BH #2A final-force RMS cannot exceed maximum"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        for key in ("bh2c_serial_gpu", "bh2b2_host_tree_gpu"):
            with self.subTest(key=key):
                receipt = receipt_for(512)
                receipt["gpu_oracles"][key]["force_rms_relative"] = 0.03
                receipt["gpu_oracles"][key]["force_max_relative"] = 0.001
                with self.assertRaisesRegex(sweep.SweepError, "force RMS cannot exceed force maximum"):
                    sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_force_maximum_has_lower_rms_bound(self):
        receipt = receipt_for(512)
        receipt["final_force"]["direct_probe_rms_relative"] = 0.0
        receipt["final_force"]["direct_probe_max_relative"] = 0.29
        with self.assertRaisesRegex(sweep.SweepError, "probe count"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["final_force"]["gpu_vs_bh2a_flat_rms_relative"] = 0.0
        receipt["final_force"]["gpu_vs_bh2a_flat_max_relative"] = 0.29
        with self.assertRaisesRegex(sweep.SweepError, "particle count"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        for key in ("bh2c_serial_gpu", "bh2b2_host_tree_gpu"):
            with self.subTest(key=key):
                receipt = receipt_for(512)
                receipt["gpu_oracles"][key]["force_rms_relative"] = 0.0
                receipt["gpu_oracles"][key]["force_max_relative"] = 0.29
                with self.assertRaisesRegex(sweep.SweepError, "sample count"):
                    sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_executed_oracle_payloads_are_required(self):
        receipt = receipt_for(512)
        receipt["gpu_oracles"] = {"status": "executed"}
        with self.assertRaisesRegex(sweep.SweepError, "bh2c_serial_gpu"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

        receipt = receipt_for(512)
        receipt["trajectory_vs_bh2a_flat_f64"] = {"status": "executed"}
        with self.assertRaisesRegex(sweep.SweepError, "state_error"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_numeric_overflow_is_normalized_to_sweep_error(self):
        receipt = receipt_for(512)
        receipt["dt_myr"] = 10 ** 400
        with self.assertRaisesRegex(sweep.SweepError, "finite number"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_tree_buffer_size_is_bound_to_particle_count(self):
        for particles in (2, 127, 128, 129, 4096, 65536):
            with self.subTest(particles=particles):
                expected = 1152 * particles + 160 * ((particles + 127) // 128) + 32
                self.assertEqual(
                    sweep.expected_parallel_tree_buffer_bytes(particles),
                    expected,
                )

        receipt = receipt_for(512)
        receipt["parallel_tree_buffer_bytes"] = 1
        with self.assertRaisesRegex(sweep.SweepError, "allocation formula"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_benchmark_scope_must_match_frozen_declaration(self):
        receipt = receipt_for(512)
        receipt["tree_build_benchmark"]["sample_scope"] = (
            "complete rebuild call excluding queue submit and GPU synchronization"
        )
        with self.assertRaisesRegex(sweep.SweepError, "frozen complete-rebuild declaration"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_parallel_median_is_recomputed_from_samples(self):
        receipt = receipt_for(512)
        receipt["tree_build_benchmark"]["parallel_samples_seconds"] = [100.0] * 7
        receipt["tree_build_benchmark"]["parallel_median_seconds"] = 0.000001
        with self.assertRaisesRegex(sweep.SweepError, "median does not match"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_serial_benchmark_summary_is_recomputed_from_samples(self):
        receipt = receipt_for(512)
        serial = receipt["tree_build_benchmark"]["bh2c_serial"]
        serial["samples_seconds"] = [50.0] * 7
        serial["median_seconds"] = 0.01
        with self.assertRaisesRegex(sweep.SweepError, "BH #2C benchmark median"):
            sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_nonfinite_or_nonpositive_benchmark_samples_are_rejected(self):
        for bad in (0.0, -1.0, float("inf"), float("nan")):
            with self.subTest(bad=bad):
                receipt = receipt_for(512)
                receipt["tree_build_benchmark"]["parallel_samples_seconds"][0] = bad
                with self.assertRaises(sweep.SweepError):
                    sweep.validate_receipt(receipt, **validation_kwargs(512))

    def test_bh2c_benchmark_must_skip_above_its_cap(self):
        receipt = receipt_for(8192)
        receipt["tree_build_benchmark"]["bh2c_serial"] = {
            "status": "executed",
            "samples_seconds": [0.01] * 7,
            "median_seconds": 0.01,
            "parallel_vs_serial_speedup": 2.0,
        }
        with self.assertRaisesRegex(sweep.SweepError, "must skip above 4096"):
            sweep.validate_receipt(receipt, **validation_kwargs(8192))


if __name__ == "__main__":
    unittest.main()

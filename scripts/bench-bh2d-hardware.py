#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Fail-closed BH #2D real-hardware scaling sweep.

This runner does not create a performance claim by itself. It executes the
existing galaxy-bh-gpu-tree-parallel verifier with --require-hardware at each
requested resident-body count, validates the emitted receipts, and writes a
manifest tying the sweep to one clean Git revision and one adapter identity.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import platform
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any

RECEIPT_SCHEMA = "galaxy.barnes-hut-parallel-gpu-tree-receipt.v1"
MANIFEST_SCHEMA = "galaxy.bh2d-hardware-scaling-manifest.v1"
MAX_PARTICLES = 65_536
BH2C_CAP = 4_096
DEFAULT_PARTICLES = "512,1024,2048,4096,8192,16384,32768,65536"


class SweepError(RuntimeError):
    pass


def parse_particles(raw: str) -> list[int]:
    try:
        values = [int(item.strip()) for item in raw.split(",") if item.strip()]
    except ValueError as exc:
        raise SweepError("--particles must be a comma-separated list of integers") from exc
    if not values:
        raise SweepError("--particles must contain at least one value")
    if any(value < 2 or value > MAX_PARTICLES for value in values):
        raise SweepError(f"every particle count must be in 2..={MAX_PARTICLES}")
    if values != sorted(values) or len(set(values)) != len(values):
        raise SweepError("--particles must be strictly increasing with no duplicates")
    return values


def run_checked(command: list[str], cwd: Path) -> str:
    completed = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise SweepError(f"command failed ({completed.returncode}): {' '.join(command)}\n{detail}")
    return completed.stdout


def git_revision(repo_root: Path) -> str:
    return run_checked(["git", "rev-parse", "HEAD"], repo_root).strip()


def is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def require_clean_source_tree(
    repo_root: Path,
    allowed_untracked_root: Path | None = None,
) -> None:
    for command in (
        ["git", "diff", "--quiet", "--"],
        ["git", "diff", "--cached", "--quiet", "--"],
    ):
        completed = subprocess.run(command, cwd=repo_root, check=False)
        if completed.returncode != 0:
            raise SweepError(
                "tracked source tree is dirty; commit or revert changes before hardware evidence capture"
            )

    raw_untracked = run_checked(
        ["git", "ls-files", "--others", "--exclude-standard", "-z"],
        repo_root,
    )
    allowed = allowed_untracked_root.resolve() if allowed_untracked_root is not None else None
    unexpected: list[str] = []
    for relative in (item for item in raw_untracked.split("\0") if item):
        candidate = (repo_root / relative).resolve()
        if allowed is not None and is_within(candidate, allowed):
            continue
        unexpected.append(relative)
    if unexpected:
        preview = ", ".join(repr(path) for path in unexpected[:8])
        suffix = "" if len(unexpected) <= 8 else f", ... (+{len(unexpected) - 8} more)"
        raise SweepError(
            "untracked files are present outside the evidence output; "
            f"commit, remove, or ignore them only after proving they cannot affect the build: {preview}{suffix}"
        )


def require_source_provenance(
    repo_root: Path,
    revision: str,
    allowed_untracked_root: Path | None = None,
) -> None:
    current = git_revision(repo_root)
    if current != revision:
        raise SweepError(
            f"source revision changed during hardware evidence capture: expected {revision}, got {current}"
        )
    require_clean_source_tree(repo_root, allowed_untracked_root)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SweepError(message)


def require_object(value: Any, name: str) -> dict[str, Any]:
    require(isinstance(value, dict), f"{name} must be an object")
    return value


def require_list(value: Any, name: str) -> list[Any]:
    require(isinstance(value, list), f"{name} must be an array")
    return value


def require_int(value: Any, name: str, *, minimum: int | None = None) -> int:
    require(isinstance(value, int) and not isinstance(value, bool), f"{name} must be an integer")
    if minimum is not None:
        require(value >= minimum, f"{name} must be >= {minimum}")
    return value


def require_number(
    value: Any,
    name: str,
    *,
    positive: bool = False,
    nonnegative: bool = False,
) -> float:
    require(
        isinstance(value, (int, float)) and not isinstance(value, bool),
        f"{name} must be numeric",
    )
    numeric = float(value)
    require(math.isfinite(numeric), f"{name} must be finite")
    if positive:
        require(numeric > 0.0, f"{name} must be positive")
    if nonnegative:
        require(numeric >= 0.0, f"{name} must be nonnegative")
    return numeric


def require_string(value: Any, name: str, *, nonempty: bool = False) -> str:
    require(isinstance(value, str), f"{name} must be a string")
    if nonempty:
        require(bool(value.strip()), f"{name} must not be empty")
    return value


def require_same_number(actual: Any, expected: float, name: str) -> float:
    numeric = require_number(actual, name)
    require(numeric == float(expected), f"{name} does not match requested workload")
    return numeric


def upper_median(values: list[float]) -> float:
    ordered = sorted(values)
    return ordered[len(ordered) // 2]


def validated_samples(value: Any, name: str, expected_count: int) -> list[float]:
    raw = require_list(value, name)
    require(len(raw) == expected_count, f"{name} sample count mismatch")
    return [
        require_number(sample, f"{name}[{index}]", positive=True)
        for index, sample in enumerate(raw)
    ]


def validate_state_error(value: Any, name: str) -> dict[str, float]:
    state = require_object(value, name)
    result = {
        "position_rms_relative_l2": require_number(
            state.get("position_rms_relative_l2"),
            f"{name}.position_rms_relative_l2",
            nonnegative=True,
        ),
        "position_max_relative": require_number(
            state.get("position_max_relative"),
            f"{name}.position_max_relative",
            nonnegative=True,
        ),
        "velocity_rms_relative_l2": require_number(
            state.get("velocity_rms_relative_l2"),
            f"{name}.velocity_rms_relative_l2",
            nonnegative=True,
        ),
        "velocity_max_relative": require_number(
            state.get("velocity_max_relative"),
            f"{name}.velocity_max_relative",
            nonnegative=True,
        ),
    }
    require(result["position_rms_relative_l2"] < 0.03, f"{name} position RMS gate failed")
    require(result["position_max_relative"] < 0.30, f"{name} position max gate failed")
    require(result["velocity_rms_relative_l2"] < 0.03, f"{name} velocity RMS gate failed")
    require(result["velocity_max_relative"] < 0.30, f"{name} velocity max gate failed")
    return result


def validate_force_errors(value: Any, name: str) -> tuple[float, float]:
    evidence = require_object(value, name)
    rms = require_number(
        evidence.get("force_rms_relative"),
        f"{name}.force_rms_relative",
        nonnegative=True,
    )
    maximum = require_number(
        evidence.get("force_max_relative"),
        f"{name}.force_max_relative",
        nonnegative=True,
    )
    require(rms < 0.04, f"{name} force RMS gate failed")
    require(maximum < 0.30, f"{name} force max gate failed")
    return rms, maximum


def adapter_identity(gpu: Any) -> str:
    info = require_object(gpu, "receipt.gpu")
    required = {
        "name": require_string(info.get("name"), "receipt.gpu.name", nonempty=True),
        "backend": require_string(info.get("backend"), "receipt.gpu.backend", nonempty=True),
        "device_type": require_string(
            info.get("device_type"), "receipt.gpu.device_type", nonempty=True
        ),
        "driver": require_string(info.get("driver"), "receipt.gpu.driver"),
        "driver_info": require_string(info.get("driver_info"), "receipt.gpu.driver_info"),
    }
    require(
        bool(required["driver"].strip() or required["driver_info"].strip()),
        "receipt.gpu must include nonempty driver or driver_info provenance",
    )
    require(info.get("software") is False, "receipt.gpu.software must be false for hardware evidence")
    return json.dumps(required, sort_keys=True, separators=(",", ":"))


def validate_receipt(
    receipt: Any,
    *,
    particles: int,
    preset: str,
    steps: int,
    dt_myr: float,
    seed: int,
    theta: float,
    softening_kpc: float,
    direct_probes: int,
    oracle_limit: int,
    benchmark_warmup: int,
    benchmark_repeats: int,
) -> dict[str, Any]:
    root = require_object(receipt, "receipt")
    require(root.get("schema") == RECEIPT_SCHEMA, "unexpected BH #2D receipt schema")
    require(root.get("status") == "complete", "BH #2D receipt is not complete")
    require(root.get("phase") == "BH-2D", "receipt phase is not BH-2D")
    require(root.get("builder") == "gpu-parallel-sparse-radix-v1", "unexpected BH #2D builder")

    require(root.get("preset") == preset, "receipt preset does not match requested workload")
    require_int(root.get("particles"), "receipt.particles")
    require(root.get("particles") == particles, "receipt particle count does not match sweep point")
    require_int(root.get("steps"), "receipt.steps")
    require(root.get("steps") == steps, "receipt step count does not match sweep configuration")
    require_same_number(root.get("dt_myr"), dt_myr, "receipt.dt_myr")
    require_int(root.get("seed"), "receipt.seed")
    require(root.get("seed") == seed, "receipt seed does not match requested workload")
    require_same_number(root.get("theta"), theta, "receipt.theta")
    require_same_number(
        root.get("softening_kpc"),
        softening_kpc,
        "receipt.softening_kpc",
    )
    require_same_number(
        root.get("simulated_time_myr"),
        dt_myr * steps,
        "receipt.simulated_time_myr",
    )

    require(root.get("measurement_class") == "hardware", "software-validation receipt rejected")
    require(
        root.get("hardware_performance_claim_allowed") is True,
        "receipt does not authorize hardware performance evidence",
    )
    require(root.get("host_tree_rebuilds") == 0, "host tree rebuild occurred")
    require(
        root.get("host_particle_readbacks_during_steps") == 0,
        "host particle readback occurred inside the evolution loop",
    )
    require(root.get("force_solves") == steps + 1, "unexpected force-solve count")
    require(root.get("evolution_tree_builds") == steps + 1, "unexpected tree-build count")

    identity = adapter_identity(root.get("gpu"))

    tree = require_object(root.get("tree"), "receipt.tree")
    require(tree.get("repeat_rebuild_matches") is True, "same-state repeat tree checksum changed")
    active_cell_count = require_int(
        tree.get("active_cell_count"),
        "receipt.tree.active_cell_count",
        minimum=1,
    )
    leaf_count = require_int(tree.get("leaf_count"), "receipt.tree.leaf_count", minimum=1)
    max_depth = require_int(tree.get("max_depth"), "receipt.tree.max_depth", minimum=0)

    force = require_object(root.get("final_force"), "receipt.final_force")
    expected_probe_count = min(direct_probes, particles)
    require_int(force.get("direct_probe_count"), "receipt.final_force.direct_probe_count")
    require(
        force.get("direct_probe_count") == expected_probe_count,
        "receipt direct-probe count does not match requested workload",
    )
    direct_rms = require_number(
        force.get("direct_probe_rms_relative"),
        "receipt.final_force.direct_probe_rms_relative",
        nonnegative=True,
    )
    direct_max = require_number(
        force.get("direct_probe_max_relative"),
        "receipt.final_force.direct_probe_max_relative",
        nonnegative=True,
    )
    require(direct_rms < 0.04, "direct-force RMS gate failed")
    require(direct_max < 0.30, "direct-force max gate failed")

    expected_oracle_status = "executed" if particles <= oracle_limit else "skipped-particle-limit"
    require(
        force.get("bh2a_flat_status") == expected_oracle_status,
        f"BH #2A final-force status mismatch: expected {expected_oracle_status}",
    )
    if expected_oracle_status == "executed":
        flat_rms = require_number(
            force.get("gpu_vs_bh2a_flat_rms_relative"),
            "receipt.final_force.gpu_vs_bh2a_flat_rms_relative",
            nonnegative=True,
        )
        flat_max = require_number(
            force.get("gpu_vs_bh2a_flat_max_relative"),
            "receipt.final_force.gpu_vs_bh2a_flat_max_relative",
            nonnegative=True,
        )
        require(flat_rms < 0.04, "BH #2A final-force RMS gate failed")
        require(flat_max < 0.30, "BH #2A final-force max gate failed")

    oracles = require_object(root.get("gpu_oracles"), "receipt.gpu_oracles")
    require(
        oracles.get("status") == expected_oracle_status,
        f"GPU oracle status mismatch: expected {expected_oracle_status}",
    )
    if expected_oracle_status == "executed":
        for key, label in (
            ("bh2c_serial_gpu", "receipt.gpu_oracles.bh2c_serial_gpu"),
            ("bh2b2_host_tree_gpu", "receipt.gpu_oracles.bh2b2_host_tree_gpu"),
        ):
            oracle = require_object(oracles.get(key), label)
            validate_state_error(oracle.get("state_error"), f"{label}.state_error")
            validate_force_errors(oracle, label)
    else:
        require(
            require_int(oracles.get("limit"), "receipt.gpu_oracles.limit") == oracle_limit,
            "GPU oracle skip limit does not match requested oracle limit",
        )

    trajectory = require_object(
        root.get("trajectory_vs_bh2a_flat_f64"),
        "receipt.trajectory_vs_bh2a_flat_f64",
    )
    require(
        trajectory.get("status") == expected_oracle_status,
        f"CPU trajectory status mismatch: expected {expected_oracle_status}",
    )
    if expected_oracle_status == "executed":
        validate_state_error(
            trajectory.get("state_error"),
            "receipt.trajectory_vs_bh2a_flat_f64.state_error",
        )
    else:
        require(
            require_int(
                trajectory.get("limit"),
                "receipt.trajectory_vs_bh2a_flat_f64.limit",
            )
            == oracle_limit,
            "CPU trajectory skip limit does not match requested oracle limit",
        )

    benchmark = require_object(root.get("tree_build_benchmark"), "receipt.tree_build_benchmark")
    require(
        require_string(
            benchmark.get("sample_scope"),
            "receipt.tree_build_benchmark.sample_scope",
            nonempty=True,
        ).startswith("complete rebuild call"),
        "benchmark sample scope does not cover the complete rebuild call",
    )
    require(
        benchmark.get("stage_timings_are_diagnostics") is True,
        "benchmark must mark stage timings as diagnostics",
    )
    require(
        require_int(benchmark.get("warmup"), "receipt.tree_build_benchmark.warmup")
        == benchmark_warmup,
        "benchmark warmup count does not match requested workload",
    )
    require(
        require_int(benchmark.get("repeats"), "receipt.tree_build_benchmark.repeats")
        == benchmark_repeats,
        "benchmark repeat count does not match requested workload",
    )

    parallel_samples = validated_samples(
        benchmark.get("parallel_samples_seconds"),
        "receipt.tree_build_benchmark.parallel_samples_seconds",
        benchmark_repeats,
    )
    parallel_median = upper_median(parallel_samples)
    producer_parallel_median = require_number(
        benchmark.get("parallel_median_seconds"),
        "receipt.tree_build_benchmark.parallel_median_seconds",
        positive=True,
    )
    require(
        producer_parallel_median == parallel_median,
        "parallel benchmark median does not match its samples",
    )

    serial = require_object(
        benchmark.get("bh2c_serial"),
        "receipt.tree_build_benchmark.bh2c_serial",
    )
    if particles <= BH2C_CAP:
        require(serial.get("status") == "executed", "BH #2C comparison should execute at this size")
        serial_samples = validated_samples(
            serial.get("samples_seconds"),
            "receipt.tree_build_benchmark.bh2c_serial.samples_seconds",
            benchmark_repeats,
        )
        serial_median = upper_median(serial_samples)
        producer_serial_median = require_number(
            serial.get("median_seconds"),
            "receipt.tree_build_benchmark.bh2c_serial.median_seconds",
            positive=True,
        )
        require(
            producer_serial_median == serial_median,
            "BH #2C benchmark median does not match its samples",
        )
        speedup = serial_median / parallel_median
        producer_speedup = require_number(
            serial.get("parallel_vs_serial_speedup"),
            "receipt.tree_build_benchmark.bh2c_serial.parallel_vs_serial_speedup",
            positive=True,
        )
        require(
            producer_speedup == speedup,
            "BH #2C speedup does not match the validated benchmark medians",
        )
    else:
        require(
            serial.get("status") == "skipped-bh2c-cap",
            "BH #2C comparison must skip above 4096",
        )
        require(
            require_int(
                serial.get("limit"),
                "receipt.tree_build_benchmark.bh2c_serial.limit",
            )
            == BH2C_CAP,
            "BH #2C benchmark skip limit is invalid",
        )
        speedup = None

    parallel_tree_buffer_bytes = require_int(
        root.get("parallel_tree_buffer_bytes"),
        "receipt.parallel_tree_buffer_bytes",
        minimum=1,
    )

    return {
        "particles": particles,
        "parallel_tree_buffer_bytes": parallel_tree_buffer_bytes,
        "active_cell_count": active_cell_count,
        "leaf_count": leaf_count,
        "max_depth": max_depth,
        "parallel_median_seconds": parallel_median,
        "bh2c_parallel_vs_serial_speedup": speedup,
        "direct_probe_rms_relative": direct_rms,
        "direct_probe_max_relative": direct_max,
        "adapter_identity": identity,
    }


def command_for(args: argparse.Namespace, particles: int, receipt: Path) -> list[str]:
    command = [
        args.cargo,
        "run",
        "--release",
        "--manifest-path",
        "runtime/Cargo.toml",
        "--locked",
        "--bin",
        "galaxy-bh-gpu-tree-parallel",
        "--",
        "--preset",
        args.preset,
        "--particles",
        str(particles),
        "--steps",
        str(args.steps),
        "--dt-myr",
        str(args.dt_myr),
        "--seed",
        str(args.seed),
        "--theta",
        str(args.theta),
        "--softening-kpc",
        str(args.softening_kpc),
        "--direct-probes",
        str(args.direct_probes),
        "--oracle-limit",
        str(args.oracle_limit),
        "--benchmark-warmup",
        str(args.benchmark_warmup),
        "--benchmark-repeats",
        str(args.benchmark_repeats),
        "--require-hardware",
        "--receipt",
        str(receipt),
    ]
    if args.adapter:
        command.extend(["--adapter", args.adapter])
    return command


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run a fail-closed real-hardware BH #2D scaling sweep"
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--particles", default=DEFAULT_PARTICLES)
    parser.add_argument("--preset", choices=("disc", "collision"), default="disc")
    parser.add_argument("--steps", type=int, default=3)
    parser.add_argument("--dt-myr", type=float, default=0.01)
    parser.add_argument("--seed", type=int, default=303)
    parser.add_argument("--theta", type=float, default=0.5)
    parser.add_argument("--softening-kpc", type=float, default=0.05)
    parser.add_argument("--direct-probes", type=int, default=12)
    parser.add_argument("--oracle-limit", type=int, default=4096)
    parser.add_argument("--benchmark-warmup", type=int, default=2)
    parser.add_argument("--benchmark-repeats", type=int, default=7)
    parser.add_argument("--adapter")
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the exact hardware commands without creating files or running cargo",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root = Path(__file__).resolve().parents[1]
    manifest_path: Path | None = None
    manifest: dict[str, Any] | None = None
    try:
        particles_list = parse_particles(args.particles)
        require(1 <= args.steps <= 256, "--steps must be in 1..=256")
        require(2 <= args.oracle_limit <= BH2C_CAP, "--oracle-limit must be in 2..=4096")
        require(1 <= args.benchmark_repeats <= 31, "--benchmark-repeats must be in 1..=31")
        require(0 <= args.benchmark_warmup <= 10, "--benchmark-warmup must be in 0..=10")

        if args.dry_run:
            for particles in particles_list:
                receipt = args.output / f"n{particles:06d}" / "receipt.json"
                print(shlex.join(command_for(args, particles, receipt)))
            return 0

        output = args.output.expanduser().resolve()
        if output.exists():
            raise SweepError(f"output directory already exists: {output}")

        revision = git_revision(repo_root)
        require_source_provenance(repo_root, revision)

        output.mkdir(parents=True)
        manifest_path = output / "manifest.json"
        manifest = {
            "schema": MANIFEST_SCHEMA,
            "status": "running",
            "claim_boundary": (
                "hardware receipts and scaling observations for the recorded source/workload/adapter only; "
                "this manifest does not by itself promote BH #2D to production"
            ),
            "source_revision": revision,
            "host": {
                "platform": platform.platform(),
                "python": sys.version.split()[0],
                "machine": platform.machine(),
            },
            "configuration": {
                "particles": particles_list,
                "preset": args.preset,
                "steps": args.steps,
                "dt_myr": args.dt_myr,
                "seed": args.seed,
                "theta": args.theta,
                "softening_kpc": args.softening_kpc,
                "direct_probes": args.direct_probes,
                "oracle_limit": args.oracle_limit,
                "benchmark_warmup": args.benchmark_warmup,
                "benchmark_repeats": args.benchmark_repeats,
                "adapter_selector": args.adapter,
            },
            "runs": [],
        }
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")

        adapter: str | None = None
        for particles in particles_list:
            require_source_provenance(repo_root, revision, output)

            run_dir = output / f"n{particles:06d}"
            run_dir.mkdir()
            receipt_path = run_dir / "receipt.json"
            log_path = run_dir / "run.log"
            command = command_for(args, particles, receipt_path)

            completed = subprocess.run(
                command,
                cwd=repo_root,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
            log_path.write_text(completed.stdout)

            require_source_provenance(repo_root, revision, output)
            if completed.returncode != 0:
                raise SweepError(
                    f"BH #2D hardware run failed at {particles} particles; see {log_path}"
                )
            if not receipt_path.is_file():
                raise SweepError(f"missing receipt after {particles}-particle run")

            receipt = json.loads(receipt_path.read_text())
            summary = validate_receipt(
                receipt,
                particles=particles,
                preset=args.preset,
                steps=args.steps,
                dt_myr=args.dt_myr,
                seed=args.seed,
                theta=args.theta,
                softening_kpc=args.softening_kpc,
                direct_probes=args.direct_probes,
                oracle_limit=args.oracle_limit,
                benchmark_warmup=args.benchmark_warmup,
                benchmark_repeats=args.benchmark_repeats,
            )
            if adapter is None:
                adapter = summary["adapter_identity"]
            elif summary["adapter_identity"] != adapter:
                raise SweepError("adapter identity changed during the scaling sweep")

            summary.update(
                {
                    "receipt": str(receipt_path.relative_to(output)),
                    "receipt_sha256": sha256_file(receipt_path),
                    "log": str(log_path.relative_to(output)),
                    "log_sha256": sha256_file(log_path),
                }
            )
            summary.pop("adapter_identity")
            manifest["runs"].append(summary)
            manifest["adapter_identity"] = adapter
            manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")

        require_source_provenance(repo_root, revision, output)
        manifest["status"] = "complete"
        manifest["completed_run_count"] = len(manifest["runs"])
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        print(manifest_path)
        return 0
    except (OSError, json.JSONDecodeError, SweepError) as exc:
        if manifest_path is not None and manifest is not None and manifest_path.parent.exists():
            manifest["status"] = "failed"
            manifest["error"] = str(exc)
            try:
                manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
            except OSError:
                pass
        print(f"BH #2D hardware sweep: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

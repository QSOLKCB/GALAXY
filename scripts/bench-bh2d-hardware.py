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
import os
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
    return completed.stdout.strip()


def git_revision(repo_root: Path) -> str:
    return run_checked(["git", "rev-parse", "HEAD"], repo_root)


def require_clean_tracked_tree(repo_root: Path) -> None:
    for command in (
        ["git", "diff", "--quiet", "--"],
        ["git", "diff", "--cached", "--quiet", "--"],
    ):
        completed = subprocess.run(command, cwd=repo_root, check=False)
        if completed.returncode != 0:
            raise SweepError(
                "tracked source tree is dirty; commit or revert changes before hardware evidence capture"
            )


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SweepError(message)


def adapter_identity(gpu: Any) -> str:
    require(isinstance(gpu, dict), "receipt.gpu must be an object")
    stable = {
        key: gpu.get(key)
        for key in ("name", "backend", "device_type", "vendor", "device", "driver", "driver_info")
        if key in gpu
    }
    return json.dumps(stable, sort_keys=True, separators=(",", ":"))


def validate_receipt(
    receipt: dict[str, Any],
    *,
    particles: int,
    steps: int,
    oracle_limit: int,
    benchmark_repeats: int,
) -> dict[str, Any]:
    require(receipt.get("schema") == RECEIPT_SCHEMA, "unexpected BH #2D receipt schema")
    require(receipt.get("status") == "complete", "BH #2D receipt is not complete")
    require(receipt.get("phase") == "BH-2D", "receipt phase is not BH-2D")
    require(receipt.get("builder") == "gpu-parallel-sparse-radix-v1", "unexpected BH #2D builder")
    require(receipt.get("particles") == particles, "receipt particle count does not match sweep point")
    require(receipt.get("steps") == steps, "receipt step count does not match sweep configuration")
    require(receipt.get("measurement_class") == "hardware", "software-validation receipt rejected")
    require(
        receipt.get("hardware_performance_claim_allowed") is True,
        "receipt does not authorize hardware performance evidence",
    )
    require(receipt.get("host_tree_rebuilds") == 0, "host tree rebuild occurred")
    require(
        receipt.get("host_particle_readbacks_during_steps") == 0,
        "host particle readback occurred inside the evolution loop",
    )
    require(receipt.get("force_solves") == steps + 1, "unexpected force-solve count")
    require(receipt.get("evolution_tree_builds") == steps + 1, "unexpected tree-build count")

    tree = receipt.get("tree")
    require(isinstance(tree, dict), "receipt.tree must be an object")
    require(tree.get("repeat_rebuild_matches") is True, "same-state repeat tree checksum changed")
    require(int(tree.get("active_cell_count", 0)) > 0, "receipt reports no active cells")

    force = receipt.get("final_force")
    require(isinstance(force, dict), "receipt.final_force must be an object")
    require(float(force.get("direct_probe_rms_relative", 1.0)) < 0.04, "direct-force RMS gate failed")
    require(float(force.get("direct_probe_max_relative", 1.0)) < 0.30, "direct-force max gate failed")

    oracles = receipt.get("gpu_oracles")
    require(isinstance(oracles, dict), "receipt.gpu_oracles must be an object")
    expected_oracle_status = "executed" if particles <= oracle_limit else "skipped-particle-limit"
    require(
        oracles.get("status") == expected_oracle_status,
        f"GPU oracle status mismatch: expected {expected_oracle_status}",
    )

    trajectory = receipt.get("trajectory_vs_bh2a_flat_f64")
    require(isinstance(trajectory, dict), "trajectory evidence must be an object")
    expected_trajectory_status = "executed" if particles <= oracle_limit else "skipped-particle-limit"
    require(
        trajectory.get("status") == expected_trajectory_status,
        f"CPU trajectory status mismatch: expected {expected_trajectory_status}",
    )

    benchmark = receipt.get("tree_build_benchmark")
    require(isinstance(benchmark, dict), "tree_build_benchmark must be an object")
    require(
        benchmark.get("stage_timings_are_diagnostics") is True,
        "benchmark must mark stage timings as diagnostics",
    )
    samples = benchmark.get("parallel_samples_seconds")
    require(
        isinstance(samples, list) and len(samples) == benchmark_repeats,
        "parallel benchmark sample count mismatch",
    )
    median = float(benchmark.get("parallel_median_seconds", -1.0))
    require(median > 0.0, "parallel rebuild median must be positive")

    serial = benchmark.get("bh2c_serial")
    require(isinstance(serial, dict), "BH #2C benchmark evidence must be an object")
    if particles <= BH2C_CAP:
        require(serial.get("status") == "executed", "BH #2C comparison should execute at this size")
        speedup = float(serial.get("parallel_vs_serial_speedup", 0.0))
        require(speedup > 0.0, "BH #2C comparison speedup must be positive")
    else:
        require(serial.get("status") == "skipped-bh2c-cap", "BH #2C comparison must skip above 4096")
        speedup = None

    gpu = receipt.get("gpu")
    identity = adapter_identity(gpu)

    return {
        "particles": particles,
        "parallel_tree_buffer_bytes": int(receipt.get("parallel_tree_buffer_bytes", 0)),
        "active_cell_count": int(tree.get("active_cell_count", 0)),
        "leaf_count": int(tree.get("leaf_count", 0)),
        "max_depth": int(tree.get("max_depth", 0)),
        "parallel_median_seconds": median,
        "bh2c_parallel_vs_serial_speedup": speedup,
        "direct_probe_rms_relative": float(force["direct_probe_rms_relative"]),
        "direct_probe_max_relative": float(force["direct_probe_max_relative"]),
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
    parser.add_argument("--softening-kpc", type=float, default=0.02)
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
        require_clean_tracked_tree(repo_root)
        revision = git_revision(repo_root)

        output.mkdir(parents=True)
        manifest_path = output / "manifest.json"
        manifest: dict[str, Any] = {
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
                steps=args.steps,
                oracle_limit=args.oracle_limit,
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

        manifest["status"] = "complete"
        manifest["completed_run_count"] = len(manifest["runs"])
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        print(manifest_path)
        return 0
    except (OSError, json.JSONDecodeError, SweepError) as exc:
        print(f"BH #2D hardware sweep: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

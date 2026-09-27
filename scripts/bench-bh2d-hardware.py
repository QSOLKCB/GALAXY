#!/usr/bin/env -S python3 -I
# SPDX-License-Identifier: Apache-2.0
"""Fail-closed BH #2D real-hardware scaling sweep.

This runner does not create a performance claim by itself. It executes the
existing galaxy-bh-gpu-tree-parallel verifier with --require-hardware at each
requested resident-body count, validates the emitted receipts, and writes a
manifest tying the sweep to one clean Git revision and one adapter identity.
"""

from __future__ import annotations

# sys is built in: fail before importing anything from a caller-controlled path.
import sys
if __name__ == "__main__" and not sys.flags.isolated:
    raise SystemExit("Hardware capture requires isolated Python: python3 -I scripts/bench-bh2d-hardware.py ...")

import argparse
import hashlib
import json
import math
import os
import platform
import re
import shlex
import shutil
import stat
import subprocess
import tomllib
from pathlib import Path
from typing import Any

RECEIPT_SCHEMA = "galaxy.barnes-hut-parallel-gpu-tree-receipt.v1"
MANIFEST_SCHEMA = "galaxy.bh2d-hardware-scaling-manifest.v1"
MAX_PARTICLES = 65_536
BH2C_CAP = 4_096
DEFAULT_PARTICLES = "512,1024,2048,4096,8192,16384,32768,65536"
BENCHMARK_SAMPLE_SCOPE = (
    "complete rebuild call including host-side uniform/bind-group setup, "
    "command encoding, submit and synchronization"
)
WORKGROUP_SIZE = 128
TREE_LEVELS = 17
TREE_BUCKET_SIZE = 4
GPU_BODY_BYTES = 32
GPU_ENTRY_BYTES = 16
GPU_CELL_BYTES = 64
TREE_META_BYTES = 32
BOUNDS_RECORD_BYTES = 16
RADIX_DIGITS = 16
SYSTEM_PATH = "/usr/bin:/bin"
BUILD_ENV_EXACT = {
    "LD_PRELOAD", "LD_AUDIT", "LD_LIBRARY_PATH", "LIBRARY_PATH", "COMPILER_PATH",
    "GCC_EXEC_PREFIX", "CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_RUSTFLAGS",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "RUSTC_BOOTSTRAP",
    "RUSTUP_TOOLCHAIN",
    "CARGO_BUILD_TARGET",
    "CARGO_INCREMENTAL",
    "CC",
    "CXX",
    "AR",
    "RANLIB",
    "CFLAGS",
    "CXXFLAGS",
    "CPPFLAGS",
    "LDFLAGS",
}
BUILD_ENV_PATTERNS = (
    re.compile(r"^CARGO_TARGET_.+_(?:RUSTFLAGS|LINKER|RUNNER)$"),
    # Cargo exposes profile configuration through CARGO_PROFILE_<name>_*.
    # Fail closed for the whole namespace so newly added profile keys cannot
    # silently change evidence-build semantics.
    re.compile(r"^CARGO_PROFILE_.+$"),
)


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


def git_invocation(command: list[str], cwd: Path) -> tuple[list[str], dict[str, str]]:
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(PATH=SYSTEM_PATH, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
    git = shutil.which("git", path=SYSTEM_PATH)
    if git is None:
        raise SweepError("system Git is required")
    # rev-parse also handles a linked worktree's .git file. No ambient selectors.
    result = subprocess.run(
        [git, "--no-replace-objects", "-C", str(cwd), "rev-parse", "--absolute-git-dir"],
        env=env,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        raise SweepError("could not locate the checkout Git directory")
    git_dir = result.stdout.decode("utf-8").strip()
    return [
        git,
        "--no-replace-objects",
        "--git-dir",
        git_dir,
        "--work-tree",
        str(cwd.resolve()),
        "-c",
        "core.fsmonitor=false",
        *command[1:],
    ], env


def run_checked_bytes(command: list[str], cwd: Path) -> bytes:
    env = None
    if command[0] == "git":
        command, env = git_invocation(command, cwd)
    completed = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).decode(
            "utf-8", errors="replace"
        ).strip()
        raise SweepError(
            f"command failed ({completed.returncode}): {' '.join(command)}\n{detail}"
        )
    return completed.stdout


def run_checked(command: list[str], cwd: Path) -> str:
    payload = run_checked_bytes(command, cwd)
    try:
        return payload.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise SweepError("command output is not valid UTF-8") from exc


def git_revision(repo_root: Path) -> str:
    return run_checked(["git", "rev-parse", "HEAD"], repo_root).strip()


def is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def path_label(path: Path, repo_root: Path) -> str:
    resolved = path.resolve()
    try:
        return str(resolved.relative_to(repo_root.resolve()))
    except ValueError:
        home = Path.home().resolve()
        try:
            return str(Path("$HOME") / resolved.relative_to(home))
        except ValueError:
            return str(resolved)


def effective_cargo_config_paths(repo_root: Path) -> list[Path]:
    candidates: list[Path] = []
    current = repo_root.resolve()
    while True:
        cargo_dir = current / ".cargo"
        candidates.extend((cargo_dir / "config.toml", cargo_dir / "config"))
        if current.parent == current:
            break
        current = current.parent

    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).expanduser()
    if not cargo_home.is_absolute():
        cargo_home = repo_root / cargo_home
    cargo_home = cargo_home.resolve()
    candidates.extend((cargo_home / "config.toml", cargo_home / "config"))

    unique: list[Path] = []
    seen: set[Path] = set()
    for candidate in candidates:
        resolved = candidate.resolve()
        if resolved not in seen and resolved.is_file():
            seen.add(resolved)
            unique.append(resolved)
    return unique


def build_environment_overrides() -> list[str]:
    names: list[str] = []
    for name, value in os.environ.items():
        if not value:
            continue
        if name in BUILD_ENV_EXACT or any(pattern.fullmatch(name) for pattern in BUILD_ENV_PATTERNS):
            names.append(name)
    return sorted(names)


def require_no_build_environment_overrides() -> None:
    overrides = build_environment_overrides()
    if overrides:
        raise SweepError(
            "build-affecting environment overrides are not allowed for hardware evidence capture: "
            + ", ".join(overrides)
        )


def resolve_executable(command: str, name: str) -> Path:
    resolved = shutil.which(command)
    if resolved is None:
        raise SweepError(f"could not resolve {name} executable: {command}")
    path = Path(os.path.abspath(resolved))
    if not path.is_file():
        raise SweepError(f"resolved {name} executable is not a file: {path}")
    return path


def tool_record(command: str, name: str, repo_root: Path) -> dict[str, Any]:
    invocation = resolve_executable(command, name)
    selected = invocation
    # Keep argv[0] semantics of proxies, but identify the actual selected tool.
    rustup = shutil.which("rustup")
    is_proxy = invocation.resolve().stem == "rustup" or (
        rustup is not None and os.path.samefile(invocation, rustup)
    )
    if is_proxy:
        rustup_command = rustup if rustup and os.path.samefile(invocation, rustup) else str(invocation.resolve())
        selected = Path(run_checked([rustup_command, "which", name], repo_root).strip())
        require(selected.is_absolute() and selected.is_file(), f"rustup did not resolve {name}")
        require(not os.path.samefile(selected, invocation), f"rustup resolved {name} to its proxy")
    flags = ["--version", "--verbose"] if name == "cargo" else ["-vV"]
    return {
        "path": path_label(selected, repo_root),
        "executable": str(selected.absolute()),
        "sha256": sha256_file(selected),
        "invocation": str(invocation),
        "proxy_sha256": sha256_file(invocation) if is_proxy else None,
        "version_verbose": run_checked([str(selected), *flags], repo_root).strip(),
    }


def toolchain_context(cargo_command: str, repo_root: Path) -> dict[str, Any]:
    require_no_build_environment_overrides()
    cargo = tool_record(cargo_command, "cargo", repo_root)
    cargo["requested"] = cargo_command
    rustc = tool_record("rustc", "rustc", repo_root)
    system_tools = {}
    for name in ("cc", "c++", "gcc", "g++", "ld", "as", "ar", "ranlib", "git"):
        path = shutil.which(name, path=SYSTEM_PATH)
        if path:
            system_tools[name] = {"path": str(Path(path).resolve()), "sha256": sha256_file(Path(path))}
    require("cc" in system_tools, "system C linker (cc) is required")
    return {"cargo": cargo, "rustc": rustc, "build_path": SYSTEM_PATH, "system_tools": system_tools}


def build_environment(context: dict[str, Any]) -> dict[str, str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("GIT_")
        and key not in BUILD_ENV_EXACT
        and not any(pattern.fullmatch(key) for pattern in BUILD_ENV_PATTERNS)
    }
    env["PATH"] = SYSTEM_PATH
    env["RUSTC"] = context["rustc"]["executable"]
    return env


def dependency_source_context(
    repo_root: Path,
    toolchain: dict[str, Any],
) -> list[dict[str, Any]]:
    """Bind the exact non-repository dependency trees Cargo will compile."""
    command = [
        toolchain["cargo"]["executable"],
        "metadata",
        "--manifest-path",
        "runtime/Cargo.toml",
        "--locked",
        "--offline",
        "--format-version",
        "1",
    ]
    completed = subprocess.run(
        command,
        cwd=repo_root,
        env=build_environment(toolchain),
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout).decode(
            "utf-8", errors="replace"
        ).strip()
        raise SweepError(f"could not resolve locked offline Cargo dependency sources: {detail}")
    try:
        metadata = json.loads(completed.stdout.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise SweepError("Cargo metadata is not valid UTF-8 JSON") from exc

    root = repo_root.resolve()
    packages = require_list(
        require_object(metadata, "Cargo metadata").get("packages"),
        "Cargo metadata packages",
    )
    result: list[dict[str, Any]] = []
    for index, value in enumerate(packages):
        package = require_object(value, f"Cargo metadata packages[{index}]")
        manifest = Path(
            require_string(
                package.get("manifest_path"),
                f"Cargo metadata packages[{index}].manifest_path",
                nonempty=True,
            )
        ).resolve()
        package_root = manifest.parent
        if is_within(package_root, root):
            continue
        require(
            package_root.is_dir(),
            f"Cargo dependency source directory is missing: {package_root}",
        )
        source = package.get("source")
        require(
            source is None or isinstance(source, str),
            f"Cargo metadata packages[{index}].source must be a string or null",
        )
        result.append(
            {
                "name": require_string(
                    package.get("name"),
                    f"Cargo metadata packages[{index}].name",
                    nonempty=True,
                ),
                "version": require_string(
                    package.get("version"),
                    f"Cargo metadata packages[{index}].version",
                    nonempty=True,
                ),
                "source": source,
                "path": path_label(package_root, repo_root),
                "tree_sha256": sha256_directory(package_root),
            }
        )
    result.sort(
        key=lambda item: (
            item["name"],
            item["version"],
            item["source"] or "",
            item["path"],
        )
    )
    return result


def reject_cargo_config_redirects(document: Any, path: Path) -> None:
    root = require_object(document, f"Cargo config {path}")

    forbidden_top_level = {
        "paths": "dependency path overrides",
        "source": "source replacement",
        "registries": "registry replacement",
        "patch": "dependency patching",
    }
    for key, description in forbidden_top_level.items():
        if key in root:
            raise SweepError(
                f"Cargo config {path} contains unsupported {description} via [{key}]"
            )

    env = root.get("env")
    if env is not None:
        require_object(env, f"Cargo config {path}.env")
        raise SweepError(
            f"Cargo config {path} contains [env] overrides; build environment injection is not allowed"
        )

    build = root.get("build")
    if build is not None:
        build = require_object(build, f"Cargo config {path}.build")
        forbidden_build = (
            "rustc",
            "rustc-wrapper",
            "rustc-workspace-wrapper",
            "rustflags",
            "rustdocflags",
        )
        for key in forbidden_build:
            if key in build:
                raise SweepError(
                    f"Cargo config {path} contains unsupported build.{key} redirect/override"
                )

    target = root.get("target")
    if target is not None:
        target = require_object(target, f"Cargo config {path}.target")
        for target_name, target_config in target.items():
            target_config = require_object(
                target_config,
                f"Cargo config {path}.target.{target_name}",
            )
            for key in ("runner", "linker", "rustflags", "rustdocflags"):
                if key in target_config:
                    raise SweepError(
                        f"Cargo config {path} contains unsupported target.{target_name}.{key} "
                        "redirect/override"
                    )


def cargo_config_context(repo_root: Path) -> list[dict[str, str]]:
    context: list[dict[str, str]] = []
    root = repo_root.resolve()
    for path in effective_cargo_config_paths(repo_root):
        if is_within(path, root):
            relative = path.relative_to(root)
            git_command, git_env = git_invocation(
                ["git", "ls-files", "--error-unmatch", "--", str(relative)], repo_root)
            tracked = subprocess.run(
                git_command, env=git_env,
                cwd=repo_root,
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            if tracked.returncode != 0:
                raise SweepError(
                    "untracked or ignored Cargo configuration affects the evidence build: "
                    f"{relative}"
                )
        payload = path.read_bytes()
        try:
            document = tomllib.loads(payload.decode("utf-8"))
        except UnicodeDecodeError as exc:
            raise SweepError(f"Cargo config is not valid UTF-8: {path}") from exc
        except tomllib.TOMLDecodeError as exc:
            raise SweepError(f"Cargo config is not valid TOML: {path}: {exc}") from exc
        reject_cargo_config_redirects(document, path)
        context.append(
            {
                "path": path_label(path, repo_root),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
        )
    return context


def require_raw_tracked_worktree_matches_index(repo_root: Path) -> None:
    records = run_checked(["git", "ls-files", "--stage", "-z"], repo_root)
    for record in (item for item in records.split("\0") if item):
        try:
            metadata, relative = record.split("\t", 1)
            mode, object_id, stage = metadata.split(" ")
        except ValueError as exc:
            raise SweepError("could not parse tracked index evidence") from exc
        if stage != "0":
            raise SweepError(
                f"tracked path has an unresolved index stage and cannot be evidence: {relative}"
            )

        candidate = repo_root / relative
        expected = run_checked_bytes(["git", "cat-file", "blob", object_id], repo_root)
        if mode == "120000":
            if not candidate.is_symlink():
                raise SweepError(
                    f"tracked source tree is dirty; symlink state changed: {relative}"
                )
            actual = os.fsencode(os.readlink(candidate))
        elif mode in ("100644", "100755"):
            if candidate.is_symlink() or not candidate.is_file():
                raise SweepError(
                    f"tracked source tree is dirty; file state changed: {relative}"
                )
            actual = candidate.read_bytes()
            executable = bool(candidate.stat().st_mode & stat.S_IXUSR)
            if executable != (mode == "100755"):
                raise SweepError(
                    f"tracked source tree is dirty; executable mode changed: {relative}"
                )
        else:
            raise SweepError(
                f"unsupported tracked Git mode {mode} during evidence capture: {relative}"
            )

        if actual != expected:
            raise SweepError(
                "tracked source tree is dirty in raw bytes; commit or revert changes "
                f"before hardware evidence capture: {relative}"
            )


def require_clean_source_tree(
    repo_root: Path,
    allowed_untracked_root: Path | None = None,
) -> None:
    flagged_records = run_checked(["git", "ls-files", "-v", "-z"], repo_root)
    flagged: list[str] = []
    for record in (item for item in flagged_records.split("\0") if item):
        if len(record) < 3 or record[1] != " ":
            raise SweepError("could not parse git index flag evidence")
        tag = record[0]
        path = record[2:]
        if tag != "H":
            flagged.append(f"{tag} {path}")
    if flagged:
        preview = ", ".join(repr(item) for item in flagged[:8])
        suffix = "" if len(flagged) <= 8 else f", ... (+{len(flagged) - 8} more)"
        raise SweepError(
            "tracked files use index flags or states that can hide worktree changes; "
            f"clear assume-unchanged/skip-worktree and restore a normal index first: {preview}{suffix}"
        )

    git_command, git_env = git_invocation(
        ["git", "diff", "--no-ext-diff", "--cached", "--quiet", "--"],
        repo_root,
    )
    completed = subprocess.run(git_command, env=git_env, cwd=repo_root, check=False)
    if completed.returncode != 0:
        raise SweepError(
            "tracked index differs from HEAD; commit or revert changes before hardware evidence capture"
        )

    # Git worktree diffs can apply repository-local clean filters. Compare the
    # raw filesystem bytes against the index blobs instead, so attributes and
    # filter commands cannot make altered compiler inputs appear clean.
    require_raw_tracked_worktree_matches_index(repo_root)

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
    expected_cargo_context: list[dict[str, str]] | None = None,
    expected_toolchain_context: dict[str, Any] | None = None,
    expected_dependency_source_context: list[dict[str, Any]] | None = None,
    cargo_command: str = "cargo",
) -> None:
    current = git_revision(repo_root)
    if current != revision:
        raise SweepError(
            f"source revision changed during hardware evidence capture: expected {revision}, got {current}"
        )
    require_clean_source_tree(repo_root, allowed_untracked_root)
    current_cargo_context = cargo_config_context(repo_root)
    if expected_cargo_context is not None and current_cargo_context != expected_cargo_context:
        raise SweepError("effective Cargo configuration changed during hardware evidence capture")
    current_toolchain_context = toolchain_context(cargo_command, repo_root)
    if (
        expected_toolchain_context is not None
        and current_toolchain_context != expected_toolchain_context
    ):
        raise SweepError("Cargo/Rust toolchain changed during hardware evidence capture")
    current_dependency_source_context = dependency_source_context(
        repo_root,
        current_toolchain_context,
    )
    if (
        expected_dependency_source_context is not None
        and current_dependency_source_context != expected_dependency_source_context
    ):
        raise SweepError(
            "Cargo dependency source cache changed during hardware evidence capture"
        )


def load_receipt(path: Path) -> Any:
    payload = path.read_bytes()
    try:
        text = payload.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise SweepError(f"receipt is not valid UTF-8: {path}") from exc
    try:
        return json.loads(text)
    except ValueError as exc:
        raise SweepError(f"receipt is not valid JSON: {path}") from exc


def sha256_directory(path: Path) -> str:
    digest = hashlib.sha256()
    root = path.resolve()
    entries = sorted(root.rglob("*"), key=lambda entry: entry.relative_to(root).as_posix())
    for entry in entries:
        relative = entry.relative_to(root)
        if entry.is_symlink():
            raise SweepError(f"Cargo dependency source contains unsupported symlink: {entry}")
        if entry.is_dir():
            continue
        require(entry.is_file(), f"Cargo dependency source contains non-file entry: {entry}")
        relative_bytes = os.fsencode(str(relative))
        file_digest = bytes.fromhex(sha256_file(entry))
        digest.update(len(relative_bytes).to_bytes(8, "big"))
        digest.update(relative_bytes)
        digest.update(file_digest)
    return digest.hexdigest()


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
    try:
        numeric = float(value)
    except (OverflowError, TypeError, ValueError) as exc:
        raise SweepError(f"{name} cannot be represented as a finite number") from exc
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


def require_checksum(value: Any, name: str) -> str:
    checksum = require_string(value, name, nonempty=True)
    require(
        re.fullmatch(r"[0-9a-f]{16}", checksum) is not None,
        f"{name} must be a 16-digit lowercase hexadecimal checksum",
    )
    return checksum


def expected_parallel_tree_buffer_bytes(particles: int) -> int:
    blocks = (particles + WORKGROUP_SIZE - 1) // WORKGROUP_SIZE
    body_bytes = particles * GPU_BODY_BYTES
    entry_bytes = particles * GPU_ENTRY_BYTES
    cell_bytes = particles * TREE_LEVELS * GPU_CELL_BYTES
    bounds_bytes = blocks * BOUNDS_RECORD_BYTES
    histogram_bytes = blocks * RADIX_DIGITS * 4
    offsets_bytes = histogram_bytes
    return (
        body_bytes
        + entry_bytes * 2
        + cell_bytes
        + TREE_META_BYTES
        + bounds_bytes * 2
        + histogram_bytes
        + offsets_bytes
    )


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
    require(
        (result["position_rms_relative_l2"] == 0.0)
        == (result["position_max_relative"] == 0.0),
        f"{name} position RMS/max zero-state mismatch",
    )
    require(
        (result["velocity_rms_relative_l2"] == 0.0)
        == (result["velocity_max_relative"] == 0.0),
        f"{name} velocity RMS/max zero-state mismatch",
    )
    require(result["position_rms_relative_l2"] < 0.03, f"{name} position RMS gate failed")
    require(result["position_max_relative"] < 0.30, f"{name} position max gate failed")
    require(result["velocity_rms_relative_l2"] < 0.03, f"{name} velocity RMS gate failed")
    require(result["velocity_max_relative"] < 0.30, f"{name} velocity max gate failed")
    return result


def validate_force_errors(
    value: Any,
    name: str,
    sample_count: int,
) -> tuple[float, float]:
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
    require(rms <= maximum, f"{name} force RMS cannot exceed force maximum")
    require(
        maximum <= rms * math.sqrt(sample_count),
        f"{name} force maximum is too large for RMS and sample count",
    )
    require(rms < 0.04, f"{name} force RMS gate failed")
    require(maximum < 0.30, f"{name} force max gate failed")
    return rms, maximum


def adapter_identity(gpu: Any, adapter_selector: str | None = None) -> str:
    info = require_object(gpu, "receipt.gpu")
    index = require_int(info.get("index"), "receipt.gpu.index", minimum=0)
    if adapter_selector is not None and re.fullmatch(r"[0-9]+", adapter_selector):
        require(
            index == int(adapter_selector),
            "receipt.gpu.index does not match the numeric adapter selector",
        )
    required = {
        "index": index,
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
    adapter_selector: str | None = None,
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

    identity = adapter_identity(root.get("gpu"), adapter_selector)

    tree = require_object(root.get("tree"), "receipt.tree")
    require(tree.get("repeat_rebuild_matches") is True, "same-state repeat tree checksum changed")
    require_checksum(
        tree.get("initial_checksum_fnv_mix64"),
        "receipt.tree.initial_checksum_fnv_mix64",
    )
    final_checksum = require_checksum(
        tree.get("final_checksum_fnv_mix64"),
        "receipt.tree.final_checksum_fnv_mix64",
    )
    repeat_checksum = require_checksum(
        tree.get("repeat_checksum_fnv_mix64"),
        "receipt.tree.repeat_checksum_fnv_mix64",
    )
    require(
        final_checksum == repeat_checksum,
        "receipt repeat tree checksum does not match the final tree checksum",
    )
    active_cell_count = require_int(
        tree.get("active_cell_count"),
        "receipt.tree.active_cell_count",
        minimum=1,
    )
    leaf_count = require_int(tree.get("leaf_count"), "receipt.tree.leaf_count", minimum=1)
    max_depth = require_int(tree.get("max_depth"), "receipt.tree.max_depth", minimum=0)
    cell_capacity = TREE_LEVELS * particles
    require(
        active_cell_count <= cell_capacity,
        "receipt.tree.active_cell_count exceeds the frozen sparse-cell capacity",
    )
    require(
        leaf_count <= active_cell_count,
        "receipt.tree.leaf_count cannot exceed active_cell_count",
    )
    internal_cell_count = active_cell_count - leaf_count
    require(
        leaf_count <= 3 * internal_cell_count + 1,
        "receipt.tree.leaf_count exceeds quadtree fan-out capacity",
    )
    require(
        leaf_count <= particles,
        "receipt.tree.leaf_count cannot exceed the resident particle count",
    )
    require(
        max_depth <= TREE_LEVELS - 1,
        "receipt.tree.max_depth exceeds the frozen Morton depth",
    )
    require(
        active_cell_count >= max_depth + 1,
        "receipt.tree.active_cell_count is too small for the reported maximum depth",
    )
    require(
        internal_cell_count >= max_depth,
        "receipt.tree has too few internal cells for the reported maximum depth",
    )
    if max_depth < TREE_LEVELS - 1:
        minimum_leaf_count = (particles + TREE_BUCKET_SIZE - 1) // TREE_BUCKET_SIZE
        require(
            leaf_count >= minimum_leaf_count,
            "receipt.tree.leaf_count is too small for the frozen bucket size below maximum depth",
        )
    if particles > TREE_BUCKET_SIZE:
        require(
            active_cell_count > leaf_count,
            "receipt.tree must contain an internal cell when particles exceed the frozen bucket size",
        )
        require(
            max_depth > 0,
            "receipt.tree.max_depth must be positive when particles exceed the frozen bucket size",
        )

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
    require(direct_rms <= direct_max, "direct-force RMS cannot exceed maximum")
    require(
        direct_max <= direct_rms * math.sqrt(expected_probe_count),
        "direct-force maximum is too large for RMS and probe count",
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
        require(
            flat_rms <= flat_max,
            "BH #2A final-force RMS cannot exceed maximum",
        )
        require(
            flat_max <= flat_rms * math.sqrt(particles),
            "BH #2A final-force maximum is too large for RMS and particle count",
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
            validate_force_errors(oracle, label, particles)
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
        )
        == BENCHMARK_SAMPLE_SCOPE,
        "benchmark sample scope does not match the frozen complete-rebuild declaration",
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
    expected_buffer_bytes = expected_parallel_tree_buffer_bytes(particles)
    require(
        parallel_tree_buffer_bytes == expected_buffer_bytes,
        "receipt.parallel_tree_buffer_bytes does not match the frozen BH #2D allocation formula",
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


def command_for(
    args: argparse.Namespace,
    particles: int,
    receipt: Path,
    target_dir: Path,
) -> list[str]:
    command = [
        args.cargo,
        "run",
        "--release",
        "--manifest-path",
        "runtime/Cargo.toml",
        "--target-dir",
        str(target_dir),
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
        help="print planned hardware commands without creating files or running Cargo",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo_root = Path(__file__).resolve().parents[1]
    manifest_path: Path | None = None
    manifest: dict[str, Any] | None = None
    try:
        particles_list = parse_particles(args.particles)
        if args.preset == "collision":
            require(
                particles_list[0] >= 4,
                "collision preset requires every particle count to be at least 4",
            )
        require(1 <= args.steps <= 256, "--steps must be in 1..=256")
        require(2 <= args.oracle_limit <= BH2C_CAP, "--oracle-limit must be in 2..=4096")
        require(1 <= args.benchmark_repeats <= 31, "--benchmark-repeats must be in 1..=31")
        require(0 <= args.benchmark_warmup <= 10, "--benchmark-warmup must be in 0..=10")

        require_number(args.dt_myr, "--dt-myr")
        require_number(args.theta, "--theta")
        require_number(args.softening_kpc, "--softening-kpc")
        require(1e-6 <= args.dt_myr <= 1, "--dt-myr must be in [1e-6, 1]")
        require(0 <= args.theta <= 2, "--theta must be in [0, 2]")
        require(0 <= args.softening_kpc <= 100, "--softening-kpc must be in [0, 100]")
        require(1 <= args.direct_probes <= 64, "--direct-probes must be in 1..=64")
        require(0 <= args.seed <= 2**64 - 1, "--seed must fit u64")

        output = args.output.expanduser().resolve()
        if args.dry_run:
            plan_args = argparse.Namespace(**vars(args))
            plan_args.cargo = str(resolve_executable(args.cargo, "cargo").absolute())
            for particles in particles_list:
                run_dir = output / f"n{particles:06d}"
                receipt = run_dir / "receipt.json"
                target_dir = run_dir / "cargo-target"
                print(shlex.join(command_for(plan_args, particles, receipt, target_dir)))
            return 0

        if output.exists():
            raise SweepError(f"output directory already exists: {output}")

        revision = git_revision(repo_root)
        require_clean_source_tree(repo_root)
        build_cargo_context = cargo_config_context(repo_root)
        build_toolchain_context = toolchain_context(args.cargo, repo_root)
        build_dependency_source_context = dependency_source_context(
            repo_root,
            build_toolchain_context,
        )
        require_source_provenance(
            repo_root,
            revision,
            expected_cargo_context=build_cargo_context,
            expected_toolchain_context=build_toolchain_context,
            expected_dependency_source_context=build_dependency_source_context,
            cargo_command=args.cargo,
        )

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
            "build_context": {
                "cargo_configuration": build_cargo_context,
                "toolchain": build_toolchain_context,
                "dependency_sources": build_dependency_source_context,
                "environment_overrides": [],
            },
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
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True, allow_nan=False) + "\n")

        adapter: str | None = None
        for particles in particles_list:
            require_source_provenance(
                repo_root,
                revision,
                output,
                expected_cargo_context=build_cargo_context,
                expected_toolchain_context=build_toolchain_context,
                expected_dependency_source_context=build_dependency_source_context,
                cargo_command=args.cargo,
            )

            run_dir = output / f"n{particles:06d}"
            run_dir.mkdir()
            receipt_path = run_dir / "receipt.json"
            log_path = run_dir / "run.log"
            target_dir = run_dir / "cargo-target"
            if target_dir.exists():
                raise SweepError(
                    f"fresh Cargo target directory unexpectedly exists before build: {target_dir}"
                )
            command = command_for(args, particles, receipt_path, target_dir)

            command[0] = build_toolchain_context["cargo"]["executable"]
            completed = subprocess.run(
                command,
                env=build_environment(build_toolchain_context),
                cwd=repo_root,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
            )
            # Preserve every output byte, including non-UTF-8 driver diagnostics.
            log_path.write_bytes(completed.stdout)

            require_source_provenance(
                repo_root,
                revision,
                output,
                expected_cargo_context=build_cargo_context,
                expected_toolchain_context=build_toolchain_context,
                expected_dependency_source_context=build_dependency_source_context,
                cargo_command=args.cargo,
            )
            if completed.returncode != 0:
                raise SweepError(
                    f"BH #2D hardware run failed at {particles} particles; see {log_path}"
                )
            if not receipt_path.is_file():
                raise SweepError(f"missing receipt after {particles}-particle run")

            receipt = load_receipt(receipt_path)
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
                adapter_selector=args.adapter,
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
                    "cargo_target_dir": str(target_dir.relative_to(output)),
                }
            )
            summary.pop("adapter_identity")
            manifest["runs"].append(summary)
            manifest["adapter_identity"] = adapter
            manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True, allow_nan=False) + "\n")

        require_source_provenance(
            repo_root,
            revision,
            output,
            expected_cargo_context=build_cargo_context,
            expected_toolchain_context=build_toolchain_context,
            expected_dependency_source_context=build_dependency_source_context,
            cargo_command=args.cargo,
        )
        manifest["status"] = "complete"
        manifest["completed_run_count"] = len(manifest["runs"])
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True, allow_nan=False) + "\n")
        print(manifest_path)
        return 0
    except (OSError, ValueError, SweepError) as exc:
        if manifest_path is not None and manifest is not None and manifest_path.parent.exists():
            manifest["status"] = "failed"
            manifest["error"] = str(exc)
            try:
                manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True, allow_nan=False) + "\n")
            except OSError:
                pass
        print(f"BH #2D hardware sweep: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Run a locked GALAXY checkout on an existing SSH-accessible GPU instance."""
# SPDX-License-Identifier: Apache-2.0
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]

def source_files(root=ROOT):
    fixed = ["runtime/Cargo.toml", "runtime/Cargo.lock", "runtime/build.rs",
             "runtime/tests/compact-reference.json", "rust/Cargo.toml", "rust/Cargo.lock",
             "rust-toolchain.toml", "data/uff/DEMO_GALAXY.csv", "data/uff/provenance.json",
             "data/uff/NOTICE", "tests/uff-reference.json", "scripts/run-gpu.sh", "scripts/bootstrap-gpu.sh", "LICENSE", "NOTICE.md"]
    files = [root / name for name in fixed]
    files += sorted((root / "runtime/src").glob("*.rs"))
    files += sorted((root / "runtime/src").glob("*.wgsl"))
    files += sorted((root / "rust/src").glob("*.rs"))
    for path in files:
        if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(root.resolve()):
            raise ValueError(f"Expected a regular source file inside the checkout: {path}")
    return files

def archive_source(archive, job, root=ROOT):
    with tarfile.open(archive, "w:gz") as tar:
        for path in source_files(root):
            tar.add(path, arcname="source/" + path.relative_to(root).as_posix(), recursive=False)
        tar.add(job, arcname="job.json", recursive=False)

def extract_results(archive, output):
    # Only regular result files in a newly created local run directory.
    destination = output.resolve()
    with tarfile.open(archive, "r:gz") as tar:
        total = 0
        for member in tar:
            path = PurePosixPath(member.name)
            # The archive uses POSIX paths; Windows must not reinterpret separators,
            # drive-qualified components, or alternate data stream syntax.
            if ("\\" in member.name or ":" in member.name or path.is_absolute()
                    or ".." in path.parts or not path.parts or path.parts[0] != "results"):
                raise ValueError("Unexpected result archive path")
            target = output.joinpath(*path.parts)
            if not target.resolve().is_relative_to(destination):
                raise ValueError("Unexpected result archive path")
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                total += member.size
                if total > 4 * 1024**3:
                    raise ValueError("Result archive exceeds the 4 GiB runner limit; retrieve it manually")
                target.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as src, target.open("xb") as dest:
                    shutil.copyfileobj(src, dest)
            else:
                raise ValueError("Result archive contains a link or special file")

def options(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True, help="SSH hostname/IP from your existing Vast.ai instance")
    parser.add_argument("--port", type=int, default=22)
    parser.add_argument("--user", default="root")
    parser.add_argument("--identity", type=Path)
    parser.add_argument("--job", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path, help="New local run directory")
    parser.add_argument("--remote-root", default="/workspace")
    parser.add_argument("--adapter", help="GPU adapter name substring")
    parser.add_argument("--bootstrap", action="store_true", help="Install build tools/Rust inside the remote Ubuntu/Debian container")
    parser.add_argument("--dry-run", action="store_true", help="Print the plan without contacting the instance")
    args = parser.parse_args(argv)
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.:-]*", args.host):
        parser.error("Invalid SSH hostname/IP")
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_-]*", args.user) or not 1 <= args.port <= 65535:
        parser.error("Invalid SSH user or port")
    if not PurePosixPath(args.remote_root).is_absolute():
        parser.error("--remote-root must be absolute")
    args.job = args.job.resolve()
    if not args.job.is_file() or args.job.stat().st_size > 65536:
        parser.error("--job must be a JSON file of at most 64 KiB")
    json.loads(args.job.read_text())
    if args.identity and not args.identity.is_file():
        parser.error("SSH identity file does not exist")
    return args

def plan(args):
    remote = str(PurePosixPath(args.remote_root) / ("galaxy-" + uuid.uuid4().hex[:16]))
    ssh = ["ssh", "-p", str(args.port), "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
           "-o", "ConnectTimeout=20", "-o", "ServerAliveInterval=30", "-o", "ServerAliveCountMax=3"]
    if args.identity:
        ssh += ["-i", str(args.identity.resolve())]
    ssh += [args.user + "@" + args.host]
    start = ["cd " + shlex.quote(remote + "/source")]
    if args.bootstrap:
        start.append("bash scripts/bootstrap-gpu.sh")
    run = ["env", "CARGO_TARGET_DIR=" + str(PurePosixPath(args.remote_root) / "galaxy-build-cache"),
           "bash", "scripts/run-gpu.sh", "run", "--job", remote + "/job.json", "--output", remote + "/results"]
    if args.adapter:
        run += ["--adapter", args.adapter]
    start.append(shlex.join(run))
    return {"remote_directory": remote, "ssh": ssh,
            "upload": "mkdir -p " + shlex.quote(remote) + " && tar -xzf - -C " + shlex.quote(remote),
            "run": " && ".join(start),
            "download": shlex.join(["tar", "-czf", "-", "-C", remote, "results"])}

def main(argv=None):
    args = options(argv)
    action = plan(args)
    if args.dry_run:
        print(json.dumps(action, indent=2))
        return 0
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"status": "uploading", "remote_directory": action["remote_directory"], "host": args.host,
              "port": args.port, "job_sha256": hashlib.sha256(args.job.read_bytes()).hexdigest()}
    def save():
        (args.output / "runner.json").write_text(json.dumps(report, indent=2) + "\n")
    save()
    print("Remote run directory:", action["remote_directory"], flush=True)
    try:
        with tempfile.TemporaryDirectory(prefix="galaxy-runner-") as temp:
            archive = Path(temp) / "source.tar.gz"
            archive_source(archive, args.job)
            report["source_archive_sha256"] = hashlib.sha256(archive.read_bytes()).hexdigest()
            with archive.open("rb") as source:
                subprocess.run(action["ssh"] + [action["upload"]], stdin=source, check=True)
            report["status"] = "running"
            save()
            # Stream output, preserve the real remote exit status, and keep a local log.
            with (args.output / "runner.log").open("w") as log:
                process = subprocess.Popen(action["ssh"] + [action["run"]], stdout=subprocess.PIPE,
                                           stderr=subprocess.STDOUT, text=True)
                try:
                    for line in process.stdout:
                        print(line, end="", flush=True)
                        log.write(line)
                    returncode = process.wait()
                except BaseException:
                    process.terminate()
                    process.wait()
                    raise
            report["remote_exit_code"] = returncode
            download = Path(temp) / "results.tar.gz"
            with download.open("wb") as sink:
                retrieval = subprocess.run(action["ssh"] + [action["download"]], stdout=sink)
            if retrieval.returncode == 0:
                extract_results(download, args.output)
            report["results_retrieved"] = retrieval.returncode == 0
            if returncode != 0 or retrieval.returncode != 0:
                raise RuntimeError("Remote run or result retrieval failed; see runner.log and the recorded remote directory")
            report["status"] = "complete"
            save()
            print("Results:", args.output / "results")
            return 0
    except BaseException as error:
        report["status"] = "interrupted" if isinstance(error, KeyboardInterrupt) else "failed"
        report["error"] = str(error)
        save()
        raise

if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print("GALAXY runner:", error, file=sys.stderr)
        sys.exit(1)

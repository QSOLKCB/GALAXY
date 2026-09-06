#!/usr/bin/env python3
"""Run a locked GALAXY checkout on an existing SSH-accessible GPU instance."""
# SPDX-License-Identifier: Apache-2.0
import argparse
import codecs
import gzip
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
MAX_DOWNLOAD_BYTES = 4 * 1024**3
MAX_RESULT_BYTES = 4 * 1024**3
MAX_TAR_METADATA_BYTES = 16 * 1024**2
MAX_LOG_BYTES = 16 * 1024**2
MAX_RESULT_MEMBERS = 4096
MAX_RESULT_PATHS = 4096
STREAM_CHUNK_BYTES = 64 * 1024

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

def _display_log(text):
    encoding = getattr(sys.stdout, "encoding", None)
    if encoding:
        text = text.encode(encoding, errors="backslashreplace").decode(encoding)
    print(text, end="", flush=True)

def _receive_output(command, destination, limit, error_message, display=False):
    # Binary chunks bound memory even if the sender never emits a newline.
    decoder = codecs.getincrementaldecoder("utf-8")("replace") if display else None
    with destination.open("xb") as sink:
        process = subprocess.Popen(command, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT if display else None)
        try:
            total = 0
            while True:
                chunk = process.stdout.read1(min(STREAM_CHUNK_BYTES, limit - total + 1))
                if not chunk:
                    break
                total += len(chunk)
                if total > limit:
                    raise ValueError(error_message)
                sink.write(chunk)
                if display:
                    _display_log(decoder.decode(chunk))
            if display:
                _display_log(decoder.decode(b"", final=True))
            return process.wait()
        except BaseException:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            raise
        finally:
            process.stdout.close()

def download_results(command, destination):
    return _receive_output(command, destination, MAX_DOWNLOAD_BYTES,
                           "Result archive exceeds the 4 GiB compressed download limit; retrieve it manually")

def run_remote(command, log):
    return _receive_output(command, log, MAX_LOG_BYTES,
                           "Remote output exceeds the 16 MiB remote log limit; inspect the recorded remote directory",
                           display=True)

class _BoundedTarReader:
    """Forward-only gzip reads, checked before tarfile can allocate metadata."""
    def __init__(self, source):
        self.source = source
        self.position = 0
        self.limit = MAX_TAR_METADATA_BYTES
        self.file_start = None
        self.file_end = None

    def read(self, size=-1):
        if size < 0:
            raise ValueError("Result archive exceeds the 64 KiB tar read limit; retrieve it manually")
        in_validated_file = (self.file_start is not None
                             and self.file_start <= self.position < self.file_end)
        if in_validated_file:
            # ExFileObject is buffered and Python 3.14 may ask for 128 KiB even
            # when the caller copies in 64 KiB chunks. The member size has
            # already passed validation, so allow the request only inside that
            # exact payload range and never let it spill into the next header.
            size = min(size, self.file_end - self.position)
        elif size > STREAM_CHUNK_BYTES:
            raise ValueError("Result archive exceeds the 64 KiB tar read limit; retrieve it manually")
        if self.position + size > self.limit:
            raise ValueError("Result archive exceeds the 16 MiB tar metadata limit; retrieve it manually")
        data = self.source.read(size)
        self.position += len(data)
        return data

    def tell(self):
        return self.position

    def seekable(self):
        return False

    def seek(self, offset, whence=0):
        if whence == 1:
            offset += self.position
        elif whence != 0:
            raise ValueError("Unexpected result archive seek")
        if offset < self.position:
            raise ValueError("Unexpected backward result archive seek")
        if offset > self.limit:
            raise ValueError("Result archive exceeds the 16 MiB tar metadata limit; retrieve it manually")
        # Never delegate to gzip.seek(): skipped padding must spend the budget too.
        while self.position < offset:
            if not self.read(min(STREAM_CHUNK_BYTES, offset - self.position)):
                raise ValueError("Truncated result archive")
        return self.position

    def allow_file(self, size):
        # Called only after a regular, non-sparse file passes the extracted-size cap.
        # Mark exactly this payload as validated so buffered ExFileObject reads may
        # exceed the metadata request cap without granting that allowance to headers.
        self.file_start = self.position
        self.file_end = self.position + size
        self.limit += size

def extract_results(archive, output):
    # Only regular result files in a newly created local run directory.
    destination = output.resolve()
    # Use r: so parser read requests reach the guard directly. r| would insert
    # a buffering layer that can assemble an oversized metadata payload first.
    with gzip.open(archive, "rb") as source, tarfile.open(
            fileobj=_BoundedTarReader(source), mode="r:") as tar:
        total = 0
        paths = set()
        for member_count, member in enumerate(tar, 1):
            if member_count > MAX_RESULT_MEMBERS:
                raise ValueError(f"Result archive exceeds the {MAX_RESULT_MEMBERS} member limit; retrieve it manually")
            path = PurePosixPath(member.name)
            # The archive uses POSIX paths; Windows must not reinterpret separators,
            # drive-qualified components, or alternate data stream syntax.
            if ("\\" in member.name or ":" in member.name or path.is_absolute()
                    or ".." in path.parts or not path.parts or path.parts[0] != "results"):
                raise ValueError("Unexpected result archive path")
            target = output.joinpath(*path.parts)
            if not target.resolve().is_relative_to(destination):
                raise ValueError("Unexpected result archive path")
            # Count implicit parent directories too, before creating any of them.
            for depth in range(1, len(path.parts) + 1):
                paths.add(path.parts[:depth])
                if len(paths) > MAX_RESULT_PATHS:
                    raise ValueError(f"Result archive exceeds the {MAX_RESULT_PATHS} path limit; retrieve it manually")
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                if member.sparse is not None or member.size < 0:
                    raise ValueError("Result archive contains a sparse file or invalid file size")
                total += member.size
                if total > MAX_RESULT_BYTES:
                    raise ValueError("Result archive exceeds the 4 GiB runner limit; retrieve it manually")
                tar.fileobj.allow_file(member.size)
                target.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as src, target.open("xb") as dest:
                    shutil.copyfileobj(src, dest, length=STREAM_CHUNK_BYTES)
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
              "port": args.port, "job_sha256": hashlib.sha256(args.job.read_bytes()).hexdigest(),
              "results_retrieved": False}
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
            returncode = run_remote(action["ssh"] + [action["run"]], args.output / "runner.log")
            report["remote_exit_code"] = returncode
            download = Path(temp) / "results.tar.gz"
            report["results_retrieved"] = False
            retrieval_code = download_results(action["ssh"] + [action["download"]], download)
            report["retrieval_exit_code"] = retrieval_code
            if retrieval_code == 0:
                extract_results(download, args.output)
                report["results_retrieved"] = True
            if returncode != 0 or retrieval_code != 0:
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

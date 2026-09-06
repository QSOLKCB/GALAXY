# SPDX-License-Identifier: Apache-2.0
import importlib.util
from contextlib import redirect_stdout
import gzip
import io
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("vast_runner", ROOT / "scripts/vast-runner.py")
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)

class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.job = self.root / "job.json"
        self.job.write_text('{"schema_version":1,"task":{"kind":"compact"}}')
        self.args = ["--host", "example.test", "--port", "12345", "--job", str(self.job), "--output", str(self.root / "output")]

    def test_dry_run_does_not_connect(self):
        with patch.object(runner.subprocess, "run") as call, patch.object(runner.subprocess, "Popen") as process:
            self.assertEqual(runner.main(self.args + ["--dry-run"]), 0)
            call.assert_not_called()
            process.assert_not_called()
        self.assertFalse((self.root / "output").exists())

    def test_remote_arguments_are_quoted_and_host_keys_checked(self):
        token = "NVIDIA; touch /tmp/unwanted"
        action = runner.plan(runner.options(self.args + ["--adapter", token]))
        self.assertIn("StrictHostKeyChecking=yes", action["ssh"])
        self.assertIn(token, shlex.split(action["run"]))
        self.assertIn("CARGO_TARGET_DIR=", action["run"])

    def test_source_archive_contains_build_inputs_and_excludes_build_outputs(self):
        path = self.root / "source.tar.gz"
        runner.archive_source(path, self.job)
        with tarfile.open(path) as tar:
            names = tar.getnames()
        self.assertIn("source/runtime/Cargo.lock", names)
        self.assertIn("source/runtime/src/kernels.wgsl", names)
        self.assertIn("source/tests/uff-reference.json", names)
        self.assertFalse(any("/target/" in name or "/.git/" in name for name in names))

    def archive(self, path, name="results/receipt.json", link=False, directory=False):
        with tarfile.open(path, "w:gz") as tar:
            entry = tarfile.TarInfo(name)
            if link:
                entry.type = tarfile.SYMTYPE
                entry.linkname = "/tmp/elsewhere"
                tar.addfile(entry)
            elif directory:
                entry.type = tarfile.DIRTYPE
                tar.addfile(entry)
            else:
                data = b'{"status":"complete"}'
                entry.size = len(data)
                tar.addfile(entry, io.BytesIO(data))

    def test_result_extraction_rejects_traversal_and_links(self):
        path = self.root / "bad.tar.gz"
        for name, link in [("../escape", False), ("results/../../escape", False), ("results/link", True)]:
            self.archive(path, name, link)
            with self.assertRaises(ValueError):
                runner.extract_results(path, self.root / "destination")
        self.assertFalse((self.root / "escape").exists())

    def test_result_extraction_rejects_windows_paths_before_filesystem_access(self):
        path = self.root / "windows-paths.tar.gz"
        names = [
            r"results/sub\..\..\..\payload",
            r"results\..\payload",
            "results/C:/payload",
            "results/C:payload",
            "results/sub/D:/payload",
            r"results/\\server\share\payload",
            r"results/\rooted\payload",
            "results/receipt.json:payload",
        ]
        for index, name in enumerate(names):
            for directory in (False, True):
                with self.subTest(name=name, directory=directory):
                    destination = self.root / f"destination-{index}-{int(directory)}"
                    self.archive(path, name, directory=directory)
                    with self.assertRaisesRegex(ValueError, "Unexpected result archive path"):
                        runner.extract_results(path, destination)
                    self.assertFalse(destination.exists())

    def test_result_extraction_accepts_nested_posix_paths(self):
        path = self.root / "nested.tar.gz"
        name = "results/nested run/receipt.json"
        self.archive(path, name)
        destination = self.root / "destination"
        runner.extract_results(path, destination)
        target = destination / "results" / "nested run" / "receipt.json"
        self.assertEqual(json.loads(target.read_text()), {"status": "complete"})
        self.assertTrue(target.resolve().is_relative_to(destination.resolve()))

    def metadata_archive(self, path, kind, size, repeats=1, payload=True):
        # Write raw headers: tarfile hides these records from member iteration.
        header = tarfile.TarInfo("././@LongLink")
        header.type = kind
        header.size = size
        entry = tarfile.TarInfo("results/receipt.json")
        data = b'{"status":"complete"}'
        entry.size = len(data)
        with gzip.open(path, "wb") as stream:
            for _ in range(repeats):
                stream.write(header.tobuf(format=tarfile.GNU_FORMAT))
                if payload:
                    value = (b"results/receipt.json\0" if kind in
                             (tarfile.GNUTYPE_LONGNAME, tarfile.GNUTYPE_LONGLINK) else b"")
                    stream.write(value.ljust(size, b"\0"))
                    stream.write(b"\0" * (-size % 512))
            stream.write(entry.tobuf())
            stream.write(data.ljust(512, b"\0"))
            stream.write(b"\0" * 1024)

    def test_result_metadata_rejected_before_oversized_decompression_request(self):
        kinds = (tarfile.XHDTYPE, tarfile.XGLTYPE, tarfile.SOLARIS_XHDTYPE,
                 tarfile.GNUTYPE_LONGNAME, tarfile.GNUTYPE_LONGLINK)
        for kind in kinds:
            for size in (64 * 1024 + 512, 1 << 40):
                with self.subTest(kind=kind, size=size):
                    archive = self.root / "metadata.tar.gz"
                    destination = self.root / "metadata"
                    # The huge declaration has no huge payload, keeping the test bounded.
                    self.metadata_archive(archive, kind, size, payload=size < 1 << 40)
                    original_read = gzip.GzipFile.read
                    requests = []
                    def read(source, amount=-1):
                        requests.append(amount)
                        self.assertLessEqual(amount, 64 * 1024)
                        self.assertGreaterEqual(amount, 0)
                        return original_read(source, amount)
                    with patch.object(gzip.GzipFile, "read", autospec=True, side_effect=read):
                        with self.assertRaisesRegex(ValueError, "tar read limit"):
                            runner.extract_results(archive, destination)
                    self.assertEqual(requests, [512])
                    self.assertFalse(destination.exists())

    def test_result_metadata_accepts_exact_read_limit(self):
        for kind in (tarfile.XHDTYPE, tarfile.XGLTYPE, tarfile.GNUTYPE_LONGNAME):
            with self.subTest(kind=kind):
                archive = self.root / "exact-metadata.tar.gz"
                destination = self.root / f"exact-metadata-{kind.decode()}"
                self.metadata_archive(archive, kind, 64 * 1024)
                runner.extract_results(archive, destination)
                self.assertEqual(json.loads((destination / "results/receipt.json").read_text()),
                                 {"status": "complete"})

    def test_result_metadata_budget_counts_hidden_headers_before_first_member(self):
        for kind in (tarfile.XGLTYPE, tarfile.GNUTYPE_LONGNAME):
            with self.subTest(kind=kind):
                archive = self.root / "hidden-headers.tar.gz"
                destination = self.root / "hidden-headers"
                self.metadata_archive(archive, kind, 512, repeats=3)
                with patch.object(runner, "MAX_TAR_METADATA_BYTES", 2048):
                    with self.assertRaisesRegex(ValueError, "tar metadata limit"):
                        runner.extract_results(archive, destination)
                self.assertFalse(destination.exists())

    def test_result_metadata_budget_preserves_gnu_and_pax_file_streaming(self):
        data = bytes(range(256)) * 1024 + b"tail"
        name = "results/" + "long-name-" * 10 + "receipt.json"
        for format in (tarfile.GNU_FORMAT, tarfile.PAX_FORMAT):
            archive = self.root / f"long-path-{format}.tar.gz"
            with tarfile.open(archive, "w:gz", format=format) as tar:
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                tar.addfile(entry, io.BytesIO(data))
                tar.addfile(tarfile.TarInfo("results/empty"))
            # End with exactly the first zero header consumed by TarFile.next().
            with tarfile.open(archive, "r:gz") as tar:
                final = tar.getmembers()[-1]
                metadata_bytes = final.offset_data + 512 - len(data)
            for allowed in (metadata_bytes, metadata_bytes - 1):
                with self.subTest(format=format, allowed=allowed):
                    destination = self.root / f"stream-{format}-{allowed}"
                    with patch.object(runner, "MAX_TAR_METADATA_BYTES", allowed):
                        if allowed == metadata_bytes:
                            runner.extract_results(archive, destination)
                            self.assertEqual((destination / name).read_bytes(), data)
                            self.assertEqual((destination / "results/empty").stat().st_size, 0)
                        else:
                            with self.assertRaisesRegex(ValueError, "tar metadata limit"):
                                runner.extract_results(archive, destination)

    def test_result_file_allowance_requires_a_validated_nonsparse_size(self):
        cases = ((tarfile.GNUTYPE_SPARSE, 0, "sparse file"),
                 (tarfile.REGTYPE, -1, "invalid (file size|offset)"),
                 (tarfile.REGTYPE, 4 * 1024**3 + 1, "4 GiB runner limit"))
        for kind, size, error in cases:
            with self.subTest(kind=kind, size=size):
                archive = self.root / "invalid-file.tar.gz"
                destination = self.root / "invalid-file"
                header = tarfile.TarInfo("results/data.csv")
                header.type = kind
                header.size = size
                with gzip.open(archive, "wb") as stream:
                    stream.write(header.tobuf(format=tarfile.GNU_FORMAT))
                    stream.write(b"\0" * 1024)
                with self.assertRaisesRegex((ValueError, tarfile.ReadError), error):
                    runner.extract_results(archive, destination)
                self.assertFalse(destination.exists())

    def test_oversized_metadata_is_not_reported_as_success(self):
        archive = self.root / "bad-results.tar.gz"
        self.metadata_archive(archive, tarfile.XHDTYPE, 64 * 1024 + 512)
        with self.assertRaisesRegex(ValueError, "tar read limit"):
            self.execute_mock_remote(0, download=archive)
        report = json.loads((self.root / "output/runner.json").read_text())
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["remote_exit_code"], 0)
        self.assertFalse(report["results_retrieved"])
        self.assertFalse((self.root / "output/results").exists())

    def test_result_extraction_caps_empty_files_and_directories(self):
        for directory in (False, True):
            with self.subTest(directory=directory):
                archive = self.root / f"many-{int(directory)}.tar.gz"
                destination = self.root / f"many-{int(directory)}"
                with tarfile.open(archive, "w:gz") as tar:
                    for index in range(3):
                        member = tarfile.TarInfo(f"results/entry-{index}")
                        if directory:
                            member.type = tarfile.DIRTYPE
                        tar.addfile(member)
                with patch.object(runner, "MAX_RESULT_MEMBERS", 2):
                    with self.assertRaisesRegex(ValueError, "member limit"):
                        runner.extract_results(archive, destination)
                self.assertEqual(len(list((destination / "results").iterdir())), 2)
                self.assertFalse((destination / "results/entry-2").exists())

    def test_result_extraction_accepts_exact_member_and_path_limits(self):
        archive = self.root / "exact-members.tar.gz"
        names = ["results", "results/empty", "results/nested", "results/nested/empty"]
        with tarfile.open(archive, "w:gz") as tar:
            for index, name in enumerate(names):
                member = tarfile.TarInfo(name)
                if index in (0, 2):
                    member.type = tarfile.DIRTYPE
                tar.addfile(member)
        destination = self.root / "exact-members"
        with patch.object(runner, "MAX_RESULT_MEMBERS", 4), patch.object(runner, "MAX_RESULT_PATHS", 4):
            runner.extract_results(archive, destination)
        self.assertEqual(len(list(destination.rglob("*"))), 4)
        self.assertEqual((destination / names[-1]).stat().st_size, 0)

    def test_result_path_limit_counts_implicit_parent_directories(self):
        for index, name in enumerate(["results/a/b/file", "results/a/b/c/file"]):
            with self.subTest(name=name):
                archive = self.root / f"parents-{index}.tar.gz"
                destination = self.root / f"parents-{index}"
                self.archive(archive, name)
                with patch.object(runner, "MAX_RESULT_PATHS", 4):
                    if index == 0:
                        runner.extract_results(archive, destination)
                        self.assertEqual(len(list(destination.rglob("*"))), 4)
                    else:
                        with self.assertRaisesRegex(ValueError, "path limit"):
                            runner.extract_results(archive, destination)
                        self.assertFalse(destination.exists())

    def test_excessive_remote_log_is_not_reported_as_success(self):
        with patch.object(runner, "MAX_LOG_BYTES", 8):
            with self.assertRaisesRegex(ValueError, "remote log limit"):
                self.execute_mock_remote(0)
        report = json.loads((self.root / "output/runner.json").read_text())
        self.assertEqual(report["status"], "failed")
        self.assertLessEqual((self.root / "output/runner.log").stat().st_size, 8)
        self.assertFalse((self.root / "output/results").exists())
        self.assertFalse(report["results_retrieved"])
        self.remote_process.terminate.assert_called_once_with()
        self.remote_process.wait.assert_called_once_with(timeout=5)
        self.assertTrue(self.remote_process.stdout.closed)
        self.download_process.wait.assert_not_called()

    def test_remote_log_preserves_exact_bytes_and_decodes_split_utf8(self):
        payload = b"ready: \xe2\x82\xac\n\xff\xe2"
        for exit_code in (0, 23):
            with self.subTest(exit_code=exit_code):
                destination = self.root / f"log-{exit_code}.log"
                console = io.StringIO()
                process = Mock(stdout=io.BytesIO(payload))
                process.wait.return_value = exit_code
                with patch.object(runner, "MAX_LOG_BYTES", len(payload)), patch.object(runner, "STREAM_CHUNK_BYTES", 3), patch.object(runner.subprocess, "Popen", return_value=process) as launch, redirect_stdout(console):
                    self.assertEqual(runner.run_remote(["ssh", "example.test"], destination), exit_code)
                self.assertEqual(destination.read_bytes(), payload)
                self.assertEqual(console.getvalue(), payload.decode("utf-8", errors="replace"))
                self.assertEqual(launch.call_args.kwargs["stderr"], subprocess.STDOUT)
                self.assertTrue(process.stdout.closed)

    def test_remote_log_stops_noisy_sender_without_newlines(self):
        destination = self.root / "noisy.log"
        limit = 128 * 1024
        real_popen = subprocess.Popen
        processes = []
        def launch(*args, **kwargs):
            process = real_popen(*args, **kwargs)
            processes.append(process)
            return process
        command = [sys.executable, "-u", "-c",
                   "import sys\nfor _ in range(128):\n    sys.stdout.buffer.write(b'x' * 32768)\n    sys.stderr.buffer.write(b'y' * 32768)\n"]
        with patch.object(runner, "MAX_LOG_BYTES", limit), patch.object(runner.subprocess, "Popen", side_effect=launch), redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(ValueError, "remote log limit"):
                runner.run_remote(command, destination)
        self.assertLessEqual(destination.stat().st_size, limit)
        self.assertIn(b"y", destination.read_bytes())
        self.assertIsNotNone(processes[0].poll())
        self.assertNotEqual(processes[0].returncode, 0)
        self.assertTrue(processes[0].stdout.closed)

    def test_download_accepts_exact_limit_and_preserves_exit_code(self):
        payload = bytes(range(64))
        for exit_code in (0, 23):
            with self.subTest(exit_code=exit_code):
                destination = self.root / f"boundary-{exit_code}.tar.gz"
                command = [sys.executable, "-c",
                           f"import sys; sys.stdout.buffer.write(bytes(range(64))); sys.exit({exit_code})"]
                with patch.object(runner, "MAX_DOWNLOAD_BYTES", len(payload)):
                    self.assertEqual(runner.download_results(command, destination), exit_code)
                self.assertEqual(destination.read_bytes(), payload)

    def test_download_terminates_oversized_sender_without_exceeding_disk_limit(self):
        destination = self.root / "oversized.tar.gz"
        limit = 128 * 1024
        real_popen = subprocess.Popen
        processes = []
        def launch(*args, **kwargs):
            process = real_popen(*args, **kwargs)
            processes.append(process)
            return process
        # A finite 8 MiB sender keeps the regression test itself bounded.
        command = [sys.executable, "-u", "-c",
                   "import sys\nfor _ in range(128):\n    sys.stdout.buffer.write(b'x' * 65536)\n"]
        with patch.object(runner, "MAX_DOWNLOAD_BYTES", limit), patch.object(runner.subprocess, "Popen", side_effect=launch):
            with self.assertRaisesRegex(ValueError, "compressed download"):
                runner.download_results(command, destination)
        self.assertLessEqual(destination.stat().st_size, limit)
        self.assertEqual(len(processes), 1)
        self.assertIsNotNone(processes[0].poll())
        self.assertNotEqual(processes[0].returncode, 0)
        self.assertTrue(processes[0].stdout.closed)

    def test_download_kills_and_reaps_sender_if_termination_times_out(self):
        process = Mock(stdout=io.BytesIO(b"overflow"))
        process.wait.side_effect = [subprocess.TimeoutExpired("ssh", 5), -9]
        with patch.object(runner, "MAX_DOWNLOAD_BYTES", 1), patch.object(runner.subprocess, "Popen", return_value=process):
            with self.assertRaisesRegex(ValueError, "compressed download"):
                runner.download_results(["ssh", "example.test"], self.root / "timeout.tar.gz")
        process.terminate.assert_called_once_with()
        process.kill.assert_called_once_with()
        self.assertEqual(process.wait.call_count, 2)
        self.assertTrue(process.stdout.closed)

    def execute_mock_remote(self, exit_code, retrieval_exit_code=0, download=None):
        if download is None:
            download = self.root / "download.tar.gz"
            self.archive(download)
        def run(command, **kwargs):
            self.assertGreater(len(kwargs["stdin"].read()), 0)
            return subprocess.CompletedProcess(command, 0)
        self.remote_process = Mock(stdout=io.BytesIO(b"remote job log\n"))
        self.remote_process.wait.return_value = exit_code
        self.download_process = Mock(stdout=io.BytesIO(download.read_bytes()))
        self.download_process.wait.return_value = retrieval_exit_code
        with patch.object(runner.subprocess, "run", side_effect=run), patch.object(runner.subprocess, "Popen", side_effect=[self.remote_process, self.download_process]):
            return runner.main(self.args)

    def test_success_retrieves_results_and_records_status(self):
        self.assertEqual(self.execute_mock_remote(0), 0)
        out = self.root / "output"
        self.assertEqual(json.loads((out / "runner.json").read_text())["status"], "complete")
        self.assertTrue((out / "results/receipt.json").is_file())
        self.assertIn("remote job log", (out / "runner.log").read_text())

    def test_remote_failure_is_not_reported_as_success(self):
        with self.assertRaises(RuntimeError):
            self.execute_mock_remote(23)
        report = json.loads((self.root / "output/runner.json").read_text())
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["remote_exit_code"], 23)
        self.assertTrue(report["results_retrieved"])

    def test_oversized_download_is_not_reported_as_success(self):
        with patch.object(runner, "MAX_DOWNLOAD_BYTES", 32):
            with self.assertRaisesRegex(ValueError, "compressed download"):
                self.execute_mock_remote(0)
        report = json.loads((self.root / "output/runner.json").read_text())
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["remote_exit_code"], 0)
        self.assertFalse(report["results_retrieved"])
        self.assertFalse((self.root / "output/results").exists())
        self.download_process.terminate.assert_called_once_with()
        self.download_process.wait.assert_called_once_with(timeout=5)
        self.assertTrue(self.download_process.stdout.closed)

    def test_download_failure_is_not_reported_as_success(self):
        with self.assertRaises(RuntimeError):
            self.execute_mock_remote(0, retrieval_exit_code=23)
        report = json.loads((self.root / "output/runner.json").read_text())
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["remote_exit_code"], 0)
        self.assertEqual(report["retrieval_exit_code"], 23)
        self.assertFalse(report["results_retrieved"])
        self.assertFalse((self.root / "output/results").exists())

if __name__ == "__main__":
    unittest.main()

# SPDX-License-Identifier: Apache-2.0
import importlib.util
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

    def execute_mock_remote(self, exit_code, retrieval_exit_code=0):
        download = self.root / "download.tar.gz"
        self.archive(download)
        def run(command, **kwargs):
            self.assertGreater(len(kwargs["stdin"].read()), 0)
            return subprocess.CompletedProcess(command, 0)
        process = Mock(stdout=["remote job log\n"])
        process.wait.return_value = exit_code
        self.download_process = Mock(stdout=io.BytesIO(download.read_bytes()))
        self.download_process.wait.return_value = retrieval_exit_code
        with patch.object(runner.subprocess, "run", side_effect=run), patch.object(runner.subprocess, "Popen", side_effect=[process, self.download_process]):
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

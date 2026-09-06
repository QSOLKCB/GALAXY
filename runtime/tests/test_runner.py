# SPDX-License-Identifier: Apache-2.0
import importlib.util
import io
import json
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

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

    def archive(self, path, name="results/receipt.json", link=False):
        with tarfile.open(path, "w:gz") as tar:
            entry = tarfile.TarInfo(name)
            if link:
                entry.type = tarfile.SYMTYPE
                entry.linkname = "/tmp/elsewhere"
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

    def execute_mock_remote(self, exit_code):
        download = self.root / "download.tar.gz"
        self.archive(download)
        def run(command, **kwargs):
            if "stdin" in kwargs:
                self.assertGreater(len(kwargs["stdin"].read()), 0)
            if "stdout" in kwargs:
                kwargs["stdout"].write(download.read_bytes())
            return subprocess.CompletedProcess(command, 0)
        class Process:
            stdout = ["remote job log\n"]
            def wait(self):
                return exit_code
        with patch.object(runner.subprocess, "run", side_effect=run), patch.object(runner.subprocess, "Popen", return_value=Process()):
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

if __name__ == "__main__":
    unittest.main()

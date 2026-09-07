#!/usr/bin/env python3
"""用本地 SSH/SCP 替身验证部署成功路径、参数转义和失败传播，不访问网络。"""

import io
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest


class DeployRemoteTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="部署 回归 ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.project = self.root / "project"
        self.remote = self.root / "remote home"
        self.mock_bin = self.root / "mock-bin"
        for directory in [self.project / "scripts", self.project / "dist", self.remote, self.mock_bin]:
            directory.mkdir(parents=True)
        self.script = self.project / "scripts" / "deploy-remote.sh"
        shutil.copy2(Path(__file__).with_name("deploy-remote.sh"), self.script)
        self.env = dict(os.environ, PATH=f"{self.mock_bin}:{os.environ['PATH']}",
                        TGE_MOCK_REMOTE=str(self.remote), HOME=str(self.remote))
        self.write_mock("ssh", """#!/usr/bin/env python3
import os, subprocess, sys
if os.environ.get('TGE_FAIL_TOOL') == 'ssh': sys.exit(73)
assert len(sys.argv) == 3, sys.argv
sys.exit(subprocess.run(['bash', '-c', sys.argv[2]], cwd=os.environ['TGE_MOCK_REMOTE']).returncode)
""")
        self.write_mock("scp", """#!/usr/bin/env python3
import os, pathlib, shutil, sys
if os.environ.get('TGE_FAIL_TOOL') == 'scp': sys.exit(73)
assert len(sys.argv) == 3, sys.argv
relative = sys.argv[2].split(':', 1)[1]
assert relative.startswith('.task-graph-editor-deploy.'), relative
shutil.copyfile(sys.argv[1], pathlib.Path(os.environ['TGE_MOCK_REMOTE']) / relative)
""")
        for tool in ["mkdir", "tar", "cp", "chmod", "mv"]:
            executable = shutil.which(tool)
            self.assertIsNotNone(executable)
            self.write_mock(tool, f"""#!/usr/bin/env bash
if [[ "${{TGE_FAIL_TOOL:-}}" == '{tool}' ]]; then exit 73; fi
exec '{executable}' "$@"
""")

    def write_mock(self, name, content):
        path = self.mock_bin / name
        path.write_text(content)
        path.chmod(0o755)

    def package(self, nested, valid=True):
        package_name = "task-graph-editor-v0.7.0-linux-x64"
        archive = self.project / "dist" / f"{package_name}.tar.gz"
        prefix = f"{package_name}/" if nested else "./"
        files = {"VERSION": b"v0.7.0"}
        if valid:
            files["task-graph-editor"] = b"new executable"
        with tarfile.open(archive, "w:gz") as output:
            for name, content in files.items():
                entry = tarfile.TarInfo(prefix + name)
                entry.size = len(content)
                output.addfile(entry, io.BytesIO(content))
        return archive

    def deploy(self, directory="~/tools", fail=None):
        env = self.env.copy()
        if fail:
            env["TGE_FAIL_TOOL"] = fail
        return subprocess.run(["bash", str(self.script), "mock-host", "linux", directory],
                              env=env, capture_output=True, text=True)

    def test_local_and_ci_archive_formats(self):
        for nested in [True, False]:
            with self.subTest(nested=nested):
                self.package(nested)
                result = self.deploy()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual((self.remote / "tools/task-graph-editor").read_bytes(), b"new executable")
                self.assertEqual((self.remote / "tools/VERSION").read_text(), "v0.7.0")
                self.assertEqual((self.remote / "tools/task-graph-editor").stat().st_mode & 0o777, 0o755)
                self.assertFalse(list(self.remote.glob(".task-graph-editor-deploy.*")))

    def test_remote_directory_is_literal_and_tilde_is_expanded(self):
        self.package(True)
        directory = "~/带 空格's $(touch TGE_QUOTE_BUG); tools"
        result = self.deploy(directory)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.remote / directory[2:] / "task-graph-editor").is_file())
        self.assertFalse(list(self.root.rglob("TGE_QUOTE_BUG")))

    def test_failures_return_original_code_and_never_report_success(self):
        self.package(True)
        installed = self.remote / "tools/task-graph-editor"
        installed.parent.mkdir()
        for tool in ["ssh", "scp", "mkdir", "tar", "cp", "chmod", "mv"]:
            with self.subTest(tool=tool):
                installed.write_bytes(b"old executable")
                result = self.deploy(fail=tool)
                self.assertEqual(result.returncode, 73, result.stdout + result.stderr)
                self.assertNotIn("部署成功", result.stdout)
                self.assertEqual(installed.read_bytes(), b"old executable")
                if tool not in ["ssh", "scp"]:
                    self.assertTrue(list(self.remote.glob(".task-graph-editor-deploy.*/archive.tar.gz")))
                self.assertFalse(list(installed.parent.glob(".task-graph-editor.*")))

    def test_missing_binary_preserves_archive_and_existing_installation(self):
        self.package(False, valid=False)
        installed = self.remote / "tools/task-graph-editor"
        installed.parent.mkdir()
        installed.write_bytes(b"old executable")
        result = self.deploy()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("产物缺少", result.stderr)
        self.assertEqual(installed.read_bytes(), b"old executable")
        self.assertNotIn("部署成功", result.stdout)
        self.assertTrue(list(self.remote.glob(".task-graph-editor-deploy.*/archive.tar.gz")))

    def test_missing_local_archive_reports_error_before_connecting(self):
        result = self.deploy(fail="ssh")
        self.assertEqual(result.returncode, 1)
        self.assertIn("没有 Linux tar.gz 产物", result.stderr)


if __name__ == "__main__":
    unittest.main()

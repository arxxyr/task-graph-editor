#!/usr/bin/env python3
"""仅使用隔离的本地仓库验证发布标签门禁，不创建真实远端标签。"""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().with_name("check-release-tag.sh")


class ReleaseTagTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="发布标签检查 空格-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "工作 仓库"
        self.origin = self.root / "远端 仓库.git"
        self.home = self.root / "隔离 主目录"
        self.home.mkdir()
        # 清除可能改变仓库、索引、配置或命令执行的外部 Git 环境设置。
        self.env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("GIT_", "BASH_FUNC_")) and key not in {"BASH_ENV", "ENV"}
        }
        self.env.update(
            HOME=str(self.home),
            USERPROFILE=str(self.home),
            XDG_CONFIG_HOME=str(self.home / "config"),
            GIT_CONFIG_NOSYSTEM="1",
            GIT_CONFIG_GLOBAL=os.devnull,
            GIT_CONFIG_SYSTEM=os.devnull,
            GIT_TERMINAL_PROMPT="0",
        )
        self.git("init", "--bare", "--initial-branch=master", str(self.origin), cwd=self.root)
        self.git("init", "--initial-branch=master", str(self.repo), cwd=self.root)
        self.git("remote", "add", "origin", str(self.origin))
        self.base = self.commit("初始提交")
        self.git("push", "origin", "master")

    def git(self, *args, cwd=None):
        return subprocess.run(
            [
                "git",
                "-c", "user.name=发布门禁测试",
                "-c", "user.email=release-test@example.invalid",
                "-c", "commit.gpgsign=false",
                "-c", "tag.gpgsign=false",
                "-c", f"core.hooksPath={os.devnull}",
                "-c", "protocol.file.allow=always",
                *args,
            ],
            cwd=cwd or self.repo,
            env=self.env,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        ).stdout.strip()

    def commit(self, message, filename="内容.txt"):
        path = self.repo / filename
        with path.open("a", encoding="utf-8") as output:
            output.write(message + "\n")
        self.git("add", "--", filename)
        self.git("commit", "-m", message)
        return self.git("rev-parse", "HEAD")

    def check_tag(self, ref, sha, *, allowed, message=None, cwd=None, shell="bash"):
        result = subprocess.run(
            [shell, str(SCRIPT), ref, sha],
            cwd=cwd or self.repo,
            env=self.env,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        diagnostic = result.stdout + result.stderr
        if allowed:
            self.assertEqual(result.returncode, 0, diagnostic)
            self.assertIn("发布标签检查通过", result.stdout)
        else:
            self.assertNotEqual(result.returncode, 0, diagnostic)
            self.assertIn("发布标签检查失败", result.stderr)
        if message:
            self.assertIn(message, diagnostic)
        return result

    def test_master_head_lightweight_tag_is_allowed(self):
        self.git("tag", "v1.0.0")
        self.check_tag("refs/tags/v1.0.0", self.base, allowed=True)

    @unittest.skipUnless(Path("/bin/bash").is_file(), "当前平台没有 /bin/bash")
    def test_system_bash_accepts_non_shallow_repository(self):
        # macOS 的系统 Bash 3.2 在 set -u 下不能展开空数组。
        self.git("tag", "v1.0.0")
        self.check_tag("refs/tags/v1.0.0", self.base, allowed=True, shell="/bin/bash")

    def test_annotated_tag_object_sha_is_peeled(self):
        self.git("tag", "-a", "v1.0.0", "-m", "附注版本")
        tag_object = self.git("rev-parse", "refs/tags/v1.0.0")
        self.assertNotEqual(tag_object, self.base)
        self.check_tag("refs/tags/v1.0.0", tag_object, allowed=True)

    def test_historical_master_tag_is_allowed(self):
        self.git("tag", "v1.0.0")
        self.commit("主线后续提交")
        self.git("push", "origin", "master")
        self.git("checkout", "--detach", "v1.0.0")
        self.check_tag("refs/tags/v1.0.0", self.base, allowed=True)

    def test_tag_merged_from_dev_is_allowed(self):
        self.git("checkout", "-b", "dev")
        tagged = self.commit("开发分支提交", "开发.txt")
        self.git("tag", "-a", "v1.1.0", "-m", "待发布版本")
        self.git("checkout", "master")
        self.commit("主线独立提交")
        self.git("merge", "--no-ff", "dev", "-m", "合并开发分支")
        self.git("push", "origin", "master")
        self.git("checkout", "--detach", tagged)
        self.check_tag("refs/tags/v1.1.0", tagged, allowed=True)

    def test_unmerged_dev_tag_is_rejected(self):
        self.git("checkout", "-b", "dev")
        tagged = self.commit("未合并开发提交")
        self.git("tag", "v1.1.0")
        self.check_tag("refs/tags/v1.1.0", tagged, allowed=False, message="尚未合并")

    def test_wrong_expected_sha_is_rejected(self):
        self.commit("新版本提交")
        self.git("tag", "v1.1.0")
        self.check_tag("refs/tags/v1.1.0", self.base, allowed=False, message="预期 SHA 不一致")

    def test_different_checkout_is_rejected(self):
        tagged = self.commit("新版本提交")
        self.git("tag", "v1.1.0")
        self.git("checkout", "--detach", self.base)
        self.check_tag("refs/tags/v1.1.0", tagged, allowed=False, message="当前 HEAD 不一致")

    def test_non_version_refs_are_rejected(self):
        self.git("tag", "release-1.0.0")
        for ref in ("refs/heads/master", "refs/tags/release-1.0.0", "v1.0.0", ""):
            with self.subTest(ref=ref):
                self.check_tag(ref, self.base, allowed=False, message="仅允许")

    def test_missing_remote_master_rejects_stale_tracking_ref(self):
        self.git("tag", "v1.0.0")
        self.git("update-ref", "-d", "refs/heads/master", cwd=self.origin)
        self.assertEqual(self.git("rev-parse", "origin/master"), self.base)
        self.check_tag("refs/tags/v1.0.0", self.base, allowed=False, message="无法从 origin 刷新")

    def test_failed_fetch_rejects_stale_tracking_ref(self):
        self.git("tag", "v1.0.0")
        self.git("remote", "set-url", "origin", str(self.root / "不存在的远端.git"))
        self.assertEqual(self.git("rev-parse", "origin/master"), self.base)
        self.check_tag("refs/tags/v1.0.0", self.base, allowed=False, message="无法从 origin 刷新")

    def test_remote_master_rewind_is_refreshed_before_ancestry_check(self):
        tagged = self.commit("曾经进入主线的提交")
        self.git("tag", "v1.1.0")
        self.git("push", "origin", "master")
        self.git("update-ref", "refs/heads/master", self.base, cwd=self.origin)
        self.assertEqual(self.git("rev-parse", "origin/master"), tagged)
        self.check_tag("refs/tags/v1.1.0", tagged, allowed=False, message="尚未合并")
        self.assertEqual(self.git("rev-parse", "origin/master"), self.base)

    def test_non_commit_tag_is_rejected(self):
        blob = self.git("rev-parse", "HEAD:内容.txt")
        self.git("tag", "v1.0.0", blob)
        self.check_tag("refs/tags/v1.0.0", self.base, allowed=False, message="未指向提交")

    def test_shallow_clone_history_is_completed(self):
        tagged = self.commit("浅克隆的主线顶端")
        self.git("tag", "v1.1.0")
        self.git("push", "origin", "master", "refs/tags/v1.1.0")
        clone = self.root / "浅克隆 工作区"
        self.git("clone", "--depth=1", "--branch=master", self.origin.as_uri(), str(clone), cwd=self.root)
        self.assertEqual(self.git("rev-parse", "--is-shallow-repository", cwd=clone), "true")
        self.check_tag("refs/tags/v1.1.0", tagged, allowed=True, cwd=clone)
        self.assertEqual(self.git("rev-parse", "--is-shallow-repository", cwd=clone), "false")


if __name__ == "__main__":
    unittest.main(verbosity=2)

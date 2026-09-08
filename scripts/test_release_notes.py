#!/usr/bin/env python3
"""仅用合成 Markdown、TOML 和隔离目录验证发布说明提取，不访问远端。"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from release_notes import extract_release_notes, release_notes


SCRIPT = Path(__file__).resolve().with_name("release_notes.py")
VERSION = "0.8.2"
TITLE = f"## [{VERSION}] - 2026-09-08"
BODY = "### 修复\n\n- 修复位姿数组选中，保留 `pick_poses[1]` 的数值精度。"
ENTRY = f"{TITLE}\n\n{BODY}\n"


class ExtractReleaseNotesTests(unittest.TestCase):
    def test_extracts_only_requested_first_middle_or_last_version(self):
        entries = {
            "0.8.3": "## [0.8.3] - 2026-09-09\n\n- 后续版本的独立修复。\n",
            VERSION: ENTRY,
            "0.8.1": "## [0.8.1] - 2026-09-07\n\n- 历史版本的独立修复。\n",
        }
        changelog = "# 更新日志\n\n## [未发布]\n\n- 尚未发布。\n\n"
        changelog += "\n".join(entries.values())
        for version, expected in entries.items():
            with self.subTest(version=version):
                self.assertEqual(extract_release_notes(changelog, version), expected)

    def test_preserves_subsections_lists_links_and_fenced_examples(self):
        body = (
            "### 修复\n\n"
            "- 保留 **强调**、[文档](https://example.invalid/docs) 和中文。\n"
            "  - 保留子列表缩进。\n\n"
            "### 示例\n\n"
            "```markdown\n"
            "## [0.8.2] - 2000-01-01\n"
            "# 代码中的标题\n"
            "```\n\n"
            "> 引用里的内容也原样保留。"
        )
        expected = f"{TITLE}\n\n{body}\n"
        self.assertEqual(extract_release_notes(expected, VERSION), expected)

    def test_accepts_crlf_trailing_blank_lines_and_missing_final_newline(self):
        for changelog in (ENTRY.replace("\n", "\r\n"), ENTRY.rstrip("\n"), ENTRY + "\n\n"):
            with self.subTest(changelog=repr(changelog)):
                self.assertEqual(extract_release_notes(changelog, VERSION), ENTRY)

    def test_stops_at_any_real_level_one_or_two_heading(self):
        for boundary in ("# 附录", "## 其他说明", "## [0.8.1] - 2026-09-07", "## [未发布]"):
            with self.subTest(boundary=boundary):
                changelog = ENTRY + f"\n{boundary}\n\n- 不属于当前版本。\n"
                self.assertEqual(extract_release_notes(changelog, VERSION), ENTRY)

    def test_accepts_up_to_three_spaces_before_version_heading(self):
        for indentation in ("", " ", "  ", "   "):
            with self.subTest(indentation=indentation):
                self.assertEqual(extract_release_notes(indentation + ENTRY, VERSION), ENTRY)

    def test_ignores_fenced_fake_version_headings(self):
        for opening, closing in (
            ("```markdown", "```"),
            ("~~~markdown", "~~~"),
            ("````", "`````"),
            ("   ~~~~示例", "  ~~~~~\t"),
        ):
            with self.subTest(opening=opening, closing=closing):
                fake = f"{opening}\n{TITLE}\n\n- 代码中的伪条目。\n{closing}\n\n"
                self.assertEqual(extract_release_notes(fake + ENTRY, VERSION), ENTRY)
                with self.assertRaises(ValueError):
                    extract_release_notes(fake, VERSION)

    def test_short_mismatched_or_suffixed_fence_does_not_end_code_block(self):
        for fake_closing in ("```", "~~~~", "```` trailing", "    ````"):
            with self.subTest(fake_closing=fake_closing):
                fake = f"````markdown\n{fake_closing}\n{TITLE}\n\n- 仍在代码中。\n````\n\n"
                self.assertEqual(extract_release_notes(fake + ENTRY, VERSION), ENTRY)

    def test_backticks_in_info_string_do_not_open_fence(self):
        # 反引号围栏的信息字符串不能再含反引号，这一行是普通文本。
        changelog = "```这里有 ` 行内代码\n\n" + ENTRY
        self.assertEqual(extract_release_notes(changelog, VERSION), ENTRY)

    def test_ignores_indented_code_block_and_quoted_version_headings(self):
        for prefix in ("    ", "\t", "> "):
            with self.subTest(prefix=repr(prefix)):
                fake = f"{prefix}{TITLE}\n{prefix}- 示例条目。\n\n"
                self.assertEqual(extract_release_notes(fake + ENTRY, VERSION), ENTRY)
                with self.assertRaises(ValueError):
                    extract_release_notes(fake, VERSION)

    def test_ignores_version_templates_inside_html_comments(self):
        for comment in (
            f"<!--\n{TITLE}\n\n- 模板正文。\n-->\n\n",
            f"<!-- {TITLE} -->\n\n",
            f"<!-- 第一段 --> <!--\n{TITLE}\n-->\n\n",
        ):
            with self.subTest(comment=comment):
                self.assertEqual(extract_release_notes(comment + ENTRY, VERSION), ENTRY)
                with self.assertRaises(ValueError):
                    extract_release_notes(comment, VERSION)

    def test_comment_tokens_inside_fenced_code_are_literal(self):
        body = BODY + "\n\n```html\n<!-- 这只是没有闭合的代码示例\n```"
        expected = f"{TITLE}\n\n{body}\n"
        self.assertEqual(extract_release_notes(expected, VERSION), expected)

    def test_comment_tokens_inside_inline_or_indented_code_are_literal(self):
        for body in (
            "- 允许用 `<!--` 演示 HTML 注释。",
            "- `<!--`",
            "- 保留 ``含 ` 的 <!-- 示例`` 和后续文字。",
            "    <!-- 这是缩进代码中的字面量",
            "\t<!-- 这也是缩进代码中的字面量",
            "- `<!--` <!-- 真实注释 --> 保留示例。",
        ):
            with self.subTest(body=body):
                expected = f"{TITLE}\n\n{body}\n"
                self.assertEqual(extract_release_notes(expected, VERSION), expected)

    def test_unclosed_fence_cannot_swallow_historical_versions(self):
        for opening in ("```markdown", "~~~"):
            with self.subTest(opening=opening):
                changelog = ENTRY + f"\n{opening}\n\n## [0.8.1] - 2026-09-07\n\n- 历史内容。\n"
                with self.assertRaises(ValueError):
                    extract_release_notes(changelog, VERSION)

    def test_unclosed_comment_cannot_swallow_historical_versions(self):
        changelog = ENTRY + "\n<!--\n## [0.8.1] - 2026-09-07\n\n- 历史内容。\n"
        with self.assertRaises(ValueError):
            extract_release_notes(changelog, VERSION)

    def test_missing_version_cannot_fall_back_to_unreleased_or_prefix_match(self):
        for other in ("未发布", "Unreleased", "0.8.20", "0.8.2-rc.1", "v0.8.2"):
            with self.subTest(other=other):
                changelog = f"## [{other}] - 2026-09-08\n\n- 其他版本的内容。\n"
                with self.assertRaises(ValueError):
                    extract_release_notes(changelog, VERSION)

    def test_duplicate_version_entries_are_rejected_even_when_separated(self):
        changelog = ENTRY + "\n## [0.8.1] - 2026-09-07\n\n- 历史条目。\n\n" + ENTRY
        with self.assertRaises(ValueError):
            extract_release_notes(changelog, VERSION)

    def test_version_requires_dated_level_two_heading(self):
        for title in (
            f"# [{VERSION}] - 2026-09-08",
            f"### [{VERSION}] - 2026-09-08",
            f"## [{VERSION}]",
            f"## [{VERSION}] 2026-09-08",
            f"## [{VERSION}] - 2026-9-8",
            f"## [{VERSION}] - 2026-09-08 多余文字",
        ):
            with self.subTest(title=title):
                with self.assertRaises(ValueError):
                    extract_release_notes(f"{title}\n\n{BODY}\n", VERSION)

    def test_invalid_calendar_dates_are_rejected_and_leap_day_is_accepted(self):
        for published in ("2026-02-29", "2026-04-31", "2026-13-01", "0000-01-01"):
            with self.subTest(published=published):
                with self.assertRaises(ValueError):
                    extract_release_notes(ENTRY.replace("2026-09-08", published), VERSION)
        leap_entry = ENTRY.replace("2026-09-08", "2024-02-29")
        self.assertEqual(extract_release_notes(leap_entry, VERSION), leap_entry)

    def test_empty_comments_headings_and_blank_code_are_not_release_notes(self):
        for body in (
            "",
            " \n\t\n",
            "### 修复\n\n#### 待整理",
            "<!-- 尚未填写 -->",
            "### 修复\n\n<!-- 尚未填写 -->\n\n---",
            "```\n\n```",
            "~~~\n\n~~~",
        ):
            with self.subTest(body=body):
                with self.assertRaises(ValueError):
                    extract_release_notes(f"{TITLE}\n\n{body}\n", VERSION)

    def test_empty_list_placeholders_are_not_release_notes(self):
        for body in ("- ", "* ", "+ ", "1. ", "- [ ] ", "- <!-- 待填写 -->"):
            with self.subTest(body=body):
                with self.assertRaises(ValueError):
                    extract_release_notes(f"{TITLE}\n\n{body}\n", VERSION)

    def test_prerelease_heading_matches_exact_version(self):
        version = "0.9.0-rc.1"
        entry = ENTRY.replace(VERSION, version)
        self.assertEqual(extract_release_notes(entry, version), entry)


class ReleaseNotesFileTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="发布说明测试 空格-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.changelog = self.root / "CHANGELOG.md"
        self.manifest = self.root / "Cargo.toml"
        self.changelog.write_text(ENTRY, encoding="utf-8")
        self.write_version(VERSION)

    def write_version(self, version):
        self.manifest.write_text(
            "[package]\nname = \"合成测试\"\nversion = " + json.dumps(version) + "\n",
            encoding="utf-8",
        )

    def cli(self, *args):
        env = os.environ.copy()
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), *map(str, args)],
            cwd=self.root,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=10,
        )

    def assert_cli_failure(self, result):
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertIn("更新日志检查失败", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_manifest_version_selects_entry_with_or_without_matching_tag(self):
        for tag in (None, f"v{VERSION}"):
            with self.subTest(tag=tag):
                self.assertEqual(release_notes(self.changelog, self.manifest, tag), ENTRY)

    def test_prerelease_manifest_and_tag_must_match(self):
        version = "0.9.0-rc.1"
        self.write_version(version)
        entry = ENTRY.replace(VERSION, version)
        self.changelog.write_text(entry, encoding="utf-8")
        self.assertEqual(release_notes(self.changelog, self.manifest, f"v{version}"), entry)
        with self.assertRaises(ValueError):
            release_notes(self.changelog, self.manifest, "v0.9.0")

    def test_rejects_mismatched_tags_including_full_ref_and_extra_prefix(self):
        for tag in ("v0.8.1", VERSION, f"vv{VERSION}", f"refs/tags/v{VERSION}", f"v{VERSION}+abc1234", ""):
            with self.subTest(tag=tag):
                with self.assertRaises(ValueError):
                    release_notes(self.changelog, self.manifest, tag)

    def test_missing_or_non_string_package_version_is_rejected(self):
        for contents in (
            "",
            "[package]\nname = '合成测试'\n",
            "[workspace.package]\nversion = '0.8.2'\n",
            "[package]\nversion = ''\n",
            "[package]\nversion = 8\n",
            "[package]\nversion = { workspace = true }\n",
            "package = '错误类型'\n",
        ):
            with self.subTest(contents=contents):
                self.manifest.write_text(contents, encoding="utf-8")
                with self.assertRaises(ValueError):
                    release_notes(self.changelog, self.manifest)

    def test_cli_defaults_print_only_current_entry(self):
        result = self.cli("--tag", f"v{VERSION}")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, ENTRY)
        self.assertEqual(result.stderr, "")

    def test_cli_check_is_silent_and_does_not_create_output(self):
        before = set(self.root.iterdir())
        result = self.cli("--check", "--tag", f"v{VERSION}")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertEqual(result.stderr, "")
        self.assertEqual(set(self.root.iterdir()), before)

    def test_cli_explicit_paths_and_output_support_unicode_and_spaces(self):
        destination = self.root / "版本 正文.md"
        changelog = self.changelog.rename(self.root / "自定义 更新日志.md")
        manifest = self.manifest.rename(self.root / "自定义 项目.toml")
        result = self.cli(
            "--tag", f"v{VERSION}", "--changelog", changelog,
            "--manifest", manifest, "--output", destination,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertEqual(result.stderr, "")
        self.assertEqual(destination.read_text(encoding="utf-8"), ENTRY)

    def test_cli_check_and_output_are_mutually_exclusive(self):
        destination = self.root / "不能创建.md"
        result = self.cli("--check", "--output", destination)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertFalse(destination.exists())

    def test_cli_validation_failure_never_creates_or_truncates_output(self):
        for exists in (False, True):
            with self.subTest(exists=exists):
                destination = self.root / f"输出-{exists}.md"
                previous = "已有发布说明不能被失败校验清空。\n"
                if exists:
                    destination.write_text(previous, encoding="utf-8")
                result = self.cli("--tag", "v0.8.1", "--output", destination)
                self.assert_cli_failure(result)
                if exists:
                    self.assertEqual(destination.read_text(encoding="utf-8"), previous)
                else:
                    self.assertFalse(destination.exists())

    def test_cli_missing_changelog_or_manifest_fails_without_traceback(self):
        missing = self.root / "不存在的文件"
        for option in ("--changelog", "--manifest"):
            with self.subTest(option=option):
                self.assert_cli_failure(self.cli(option, missing))

    def test_cli_invalid_toml_and_package_type_fail_without_traceback(self):
        for contents in ("[package", "package = '错误类型'\n", "[package]\nversion = 8\n"):
            with self.subTest(contents=contents):
                self.manifest.write_text(contents, encoding="utf-8")
                self.assert_cli_failure(self.cli("--check"))

    def test_cli_invalid_utf8_changelog_fails_without_traceback(self):
        self.changelog.write_bytes(b"\xff\xfe")
        self.assert_cli_failure(self.cli("--check"))

    def test_cli_output_write_failure_is_reported(self):
        self.assert_cli_failure(self.cli("--output", self.root))


if __name__ == "__main__":
    unittest.main(verbosity=2)

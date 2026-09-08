#!/usr/bin/env python3
"""从更新日志提取唯一版本条目，作为 GitHub Release 正文；需要 Python 3.11+。"""

import argparse
from datetime import date
from pathlib import Path
import re
import sys
import tomllib


HEADING = re.compile(r"^ {0,3}(#{1,2})[ \t]+(.+?)\s*$")
VERSION_HEADING = re.compile(r"\[([^\]]+)\] - (\d{4}-\d{2}-\d{2})")
FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})(.*)$")


def without_comments(line, in_comment):
    """移除真正的注释，保留行内代码中作为示例的注释标记。"""
    visible = ""
    remaining = line
    while remaining:
        if in_comment:
            _, end, remaining = remaining.partition("-->")
            in_comment = not bool(end)
            continue
        marker = re.search(r"<!--|`+", remaining)
        if marker is None:
            visible += remaining
            break
        visible += remaining[:marker.start()]
        remaining = remaining[marker.end():]
        if marker[0] == "<!--":
            in_comment = True
            continue
        delimiter = marker[0]
        closing = re.search(r"(?<!`)" + re.escape(delimiter) + r"(?!`)", remaining)
        visible += delimiter
        if closing is not None:
            visible += remaining[:closing.end()]
            remaining = remaining[closing.end():]
    return visible, in_comment


def markdown_lines(lines):
    """扫描约定的 Markdown 条目，区分正文、代码和 HTML 注释。"""
    fence = None
    in_comment = False
    for index, line in enumerate(lines):
        if fence is not None:
            if re.fullmatch(r" {0,3}" + re.escape(fence[0]) + "{" + str(len(fence)) + r",}[ \t]*", line):
                fence = None
            yield index, line, True
            continue
        if not in_comment and (line.startswith("    ") or line.startswith("\t")):
            yield index, line, True
            continue
        opening = FENCE.match(line)
        if not in_comment and opening and not (opening[1][0] == "`" and "`" in opening[2]):
            fence = opening[1]
            yield index, line, True
            continue
        visible, in_comment = without_comments(line, in_comment)
        yield index, visible, False
    if fence is not None or in_comment:
        raise ValueError("更新日志存在未闭合的代码围栏或 HTML 注释，无法可靠确定版本边界")


def headings(lines):
    """只遍历代码和 HTML 注释之外的一级、二级标题。"""
    for index, visible, is_code in markdown_lines(lines):
        if is_code:
            continue
        heading = HEADING.match(visible)
        if heading:
            yield index, heading[1], heading[2]


def has_content(body):
    """空标题、注释、列表和任务占位都不算实际的发布说明。"""
    for _, line, is_code in markdown_lines(body.splitlines()):
        text = line.strip()
        if FENCE.match(line):
            continue
        if is_code and text:
            return True
        if re.match(r"^#{1,6}(?:\s|$)", text):
            continue
        text = re.sub(r"^(?:[-+*]|\d+[.)])(?:[ \t]+|$)", "", text).strip()
        text = re.sub(r"^\[[ xX]\](?:[ \t]+|$)", "", text).strip()
        if text.strip(" *_-`~>"):
            return True
    return False


def extract_release_notes(changelog, version):
    lines = changelog.splitlines()
    boundaries = list(headings(lines))
    matches = []
    for position, (start, level, title) in enumerate(boundaries):
        if level != "##" or not re.match(r"\[" + re.escape(version) + r"\](?:\s|$)", title):
            continue
        end = boundaries[position + 1][0] if position + 1 < len(boundaries) else len(lines)
        matches.append((start, end, title))

    if len(matches) != 1:
        raise ValueError(f"版本 {version} 必须有且只有一个条目，实际找到 {len(matches)} 个")
    start, end, title = matches[0]
    heading = VERSION_HEADING.fullmatch(title)
    if not heading:
        raise ValueError(f"版本标题必须采用：## [{version}] - YYYY-MM-DD")
    try:
        date.fromisoformat(heading[2])
    except ValueError as error:
        raise ValueError(f"版本 {version} 的发布日期无效：{heading[2]}") from error

    # 只裁剪边缘的空行，不能破坏首行缩进代码或末行的 Markdown 换行标记。
    body_start = start + 1
    while body_start < end and not lines[body_start].strip():
        body_start += 1
    while end > body_start and not lines[end - 1].strip():
        end -= 1
    body = "\n".join(lines[body_start:end])
    if not has_content(body):
        raise ValueError(f"版本 {version} 的更新日志正文不能为空")
    return f"## {title}\n\n{body}\n"


def release_notes(changelog_path, manifest_path, tag=None):
    with manifest_path.open("rb") as manifest_file:
        manifest = tomllib.load(manifest_file)
    package = manifest.get("package")
    version = package.get("version") if isinstance(package, dict) else None
    if not isinstance(version, str) or not version:
        raise ValueError("Cargo.toml 必须声明 package.version 字符串")
    if tag is not None and tag != f"v{version}":
        raise ValueError(f"标签 {tag!r} 与 Cargo.toml 版本不一致，预期 v{version}")
    return extract_release_notes(changelog_path.read_text(encoding="utf-8"), version)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", help="发布标签，必须为 v{Cargo.toml 中的版本}")
    parser.add_argument("--changelog", type=Path, default=Path("CHANGELOG.md"))
    parser.add_argument("--manifest", type=Path, default=Path("Cargo.toml"))
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--output", type=Path, help="将正文写入指定文件；默认输出到标准输出")
    output.add_argument("--check", action="store_true", help="只校验，不输出正文或写入文件")
    args = parser.parse_args()
    try:
        notes = release_notes(args.changelog, args.manifest, args.tag)
        if args.output is not None:
            # 全部检查完成后才写文件，失败时不会生成空白发布说明。
            args.output.write_text(notes, encoding="utf-8")
        elif not args.check:
            sys.stdout.write(notes)
    except (OSError, ValueError) as error:
        print(f"更新日志检查失败：{error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

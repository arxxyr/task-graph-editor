#!/usr/bin/env bash
# 只允许发布已经进入远端 master 的版本标签。
set -euo pipefail

fail() {
    printf '发布标签检查失败：%s\n' "$1" >&2
    exit 1
}

if [[ $# -ne 2 ]]; then
    fail '需要两个参数：标签引用和预期提交 SHA。'
fi

tag_ref=$1
expected_sha=$2

case "$tag_ref" in
    refs/tags/v*) ;;
    *) fail '仅允许 refs/tags/v* 形式的版本标签。' ;;
esac
git check-ref-format "$tag_ref" >/dev/null 2>&1 || fail '标签引用格式无效。'

if [[ ! "$expected_sha" =~ ^([[:xdigit:]]{40}|[[:xdigit:]]{64})$ ]]; then
    fail '预期 SHA 必须是完整的 Git 对象哈希。'
fi

# 附注标签的对象哈希和轻量标签的提交哈希都统一解引用到提交。
tag_commit=$(git rev-parse --verify --end-of-options "${tag_ref}^{commit}" 2>/dev/null) \
    || fail '版本标签不存在或未指向提交。'
expected_commit=$(git rev-parse --verify --end-of-options "${expected_sha}^{commit}" 2>/dev/null) \
    || fail '预期 SHA 不存在或未指向提交。'
head_commit=$(git rev-parse --verify 'HEAD^{commit}' 2>/dev/null) \
    || fail '当前检出位置未指向提交。'

[[ "$tag_commit" == "$expected_commit" ]] || fail '版本标签与预期 SHA 不一致。'
[[ "$tag_commit" == "$head_commit" ]] || fail '版本标签与当前 HEAD 不一致。'

shallow=$(git rev-parse --is-shallow-repository) || fail '无法确定仓库历史是否完整。'
fetch_args=(--no-tags --no-recurse-submodules)
case "$shallow" in
    true) fetch_args+=(--unshallow) ;;
    false) ;;
    *) fail '无法确定仓库历史是否完整。' ;;
esac

# 强制刷新指定引用；远端缺失或获取失败时，绝不能使用已有的跟踪引用。
if ! git fetch "${fetch_args[@]}" \
    origin '+refs/heads/master:refs/remotes/origin/master'; then
    fail '无法从 origin 刷新 master，已停止发布检查。'
fi

shallow=$(git rev-parse --is-shallow-repository) || fail '无法确认获取后的仓库历史。'
[[ "$shallow" == false ]] || fail '仓库历史仍不完整，不能判断标签是否已进入 master。'
master_commit=$(git rev-parse --verify 'refs/remotes/origin/master^{commit}' 2>/dev/null) \
    || fail '刚获取的 origin/master 未指向有效提交。'

if git merge-base --is-ancestor "$tag_commit" "$master_commit"; then
    printf '发布标签检查通过：%s 已包含于最新 origin/master。\n' "$tag_ref"
else
    status=$?
    case "$status" in
        1) fail '版本标签尚未合并到最新 origin/master。' ;;
        *) fail '无法验证版本标签与 origin/master 的提交关系。' ;;
    esac
fi

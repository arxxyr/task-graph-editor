#!/usr/bin/env bash
# 将 Linux tar.gz 产物部署到远程主机；兼容本地打包目录与 CI 的平铺格式。
# 用法：./scripts/deploy-remote.sh <host> [user] [remote_dir]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
DEPLOY_HOST="${1:?用法: $0 <host> [user] [remote_dir]}"
DEPLOY_USER="${2:-linux}"
REMOTE_DIR="${3:-/home/${DEPLOY_USER}/tools}"

if [[ $# -gt 3 || -z "$REMOTE_DIR" || ! "$DEPLOY_USER" =~ ^[a-zA-Z0-9_][a-zA-Z0-9_.-]*$ || "$DEPLOY_HOST" == -* || "$DEPLOY_HOST" == *[$'\r\n\t ']* ]]; then
    echo "错误: 主机、用户或远程目录参数无效" >&2
    exit 1
fi
DESTINATION="${DEPLOY_USER}@${DEPLOY_HOST}"

# 不解析 ls 输出；目录有空格、文件名包含换行时仍按单个路径处理。
shopt -s nullglob
ARCHIVE=""
for candidate in "${PROJECT_DIR}"/dist/task-graph-editor-*-linux-*.tar.gz; do
    if [[ -f "$candidate" && ( -z "$ARCHIVE" || "$candidate" -nt "$ARCHIVE" ) ]]; then
        ARCHIVE="$candidate"
    fi
done
if [[ -z "$ARCHIVE" ]]; then
    echo "错误: dist/ 目录下没有 Linux tar.gz 产物，请先运行 scripts/build-release.sh" >&2
    exit 1
fi
ARCHIVE_NAME="${ARCHIVE##*/}"

# ssh 会把参数重新交给远端 shell；单引号转义必须在本地完成，不能依赖 argv 边界。
quote_remote_arg() {
    printf "'%s'" "${1//\'/\'\\\'\'}"
}

echo "==> 部署 ${ARCHIVE_NAME} 到 ${DESTINATION}:${REMOTE_DIR}"
# 上传使用仅含安全字符的私有目录，兼容 scp 的 SFTP 与旧 SCP 模式。
# 远程安装目录不放进 scp 路径，避免其两种协议对空格/引号的不同解释。
UPLOAD_DIR=$(ssh "$DESTINATION" "mktemp -d .task-graph-editor-deploy.XXXXXXXX")
if [[ ! "$UPLOAD_DIR" =~ ^\.task-graph-editor-deploy\.[a-zA-Z0-9]+$ ]]; then
    echo "错误: 远程临时目录返回值无效" >&2
    exit 1
fi

echo "==> 上传压缩包 ..."
scp "$ARCHIVE" "${DESTINATION}:${UPLOAD_DIR}/archive.tar.gz"

echo "==> 解压并安装 ..."
# 参数有意在本地按远程 shell 语法转义后再发送。
# shellcheck disable=SC2029
ssh "$DESTINATION" "bash -s -- $(quote_remote_arg "$REMOTE_DIR") $(quote_remote_arg "$UPLOAD_DIR") $(quote_remote_arg "${ARCHIVE_NAME%.tar.gz}")" <<'REMOTE_EOF'
# 本地严格模式不会穿过 SSH，远端必须单独启用。
set -euo pipefail

deploy_dir=$1
upload_dir=$2
package_name=$3
case "$deploy_dir" in
    '~') deploy_dir=$HOME ;;
    '~/'*) deploy_dir="$HOME/${deploy_dir#\~/}" ;;
esac

mkdir -p -- "$deploy_dir"
mkdir -- "$upload_dir/payload"
tar xzf "$upload_dir/archive.tar.gz" -C "$upload_dir/payload"

# CI 将文件放在归档根；本地打包脚本使用与归档同名的顶层目录。
payload="$upload_dir/payload"
if [[ ! -f "$payload/task-graph-editor" ]]; then
    payload="$payload/$package_name"
fi
if [[ ! -f "$payload/task-graph-editor" || ! -f "$payload/VERSION" ]]; then
    echo "错误: 产物缺少 task-graph-editor 或 VERSION；上传包保留在 $upload_dir" >&2
    exit 1
fi

# 复制、chmod 均完成后才原子替换二进制；这些步骤失败不会破坏已安装版本。
staged_binary=""
staged_version=""
cleanup_staging() {
    if [[ -n "$staged_binary" ]]; then rm -f -- "$staged_binary"; fi
    if [[ -n "$staged_version" ]]; then rm -f -- "$staged_version"; fi
}
trap cleanup_staging EXIT
staged_binary=$(mktemp "$deploy_dir/.task-graph-editor.XXXXXXXX")
staged_version=$(mktemp "$deploy_dir/.VERSION.XXXXXXXX")
cp -- "$payload/task-graph-editor" "$staged_binary"
cp -- "$payload/VERSION" "$staged_version"
chmod 755 "$staged_binary"
chmod 644 "$staged_version"
mv -f -- "$staged_binary" "$deploy_dir/task-graph-editor"
staged_binary=""
mv -f -- "$staged_version" "$deploy_dir/VERSION"
staged_version=""

# 只在全部安装步骤成功后清理上传目录；失败保留归档，SSH 原样返回非零状态。
rm -rf -- "$upload_dir"
printf '==> 部署完成: %s/task-graph-editor\n' "$deploy_dir"
REMOTE_EOF

echo "==> 部署成功"
echo "    远程路径: ${REMOTE_DIR}/task-graph-editor"

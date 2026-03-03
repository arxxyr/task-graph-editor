#!/usr/bin/env bash
# 远程部署脚本 - 将打包好的二进制部署到目标机器
#
# 用法:
#   ./scripts/deploy-remote.sh <host> [user] [remote_dir]
#
# 示例:
#   ./scripts/deploy-remote.sh 192.168.1.100
#   ./scripts/deploy-remote.sh 192.168.1.100 linux /home/linux/tools

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ---- 参数 ----
HOST="${1:?用法: $0 <host> [user] [remote_dir]}"
USER="${2:-linux}"
REMOTE_DIR="${3:-/home/${USER}/tools}"

# ---- 找到最新的打包产物 ----
DIST_DIR="${PROJECT_DIR}/dist"
ARCHIVE=$(ls -t "${DIST_DIR}"/task-graph-editor-*.tar.gz 2>/dev/null | head -1)
if [ -z "${ARCHIVE}" ]; then
    echo "错误: dist/ 目录下没有找到打包产物"
    echo "请先运行: ./scripts/build-release.sh"
    exit 1
fi

ARCHIVE_NAME=$(basename "${ARCHIVE}")
echo "==> 部署 ${ARCHIVE_NAME} 到 ${USER}@${HOST}:${REMOTE_DIR}"

# ---- 上传 ----
echo "==> 创建远程目录 ..."
ssh "${USER}@${HOST}" "mkdir -p ${REMOTE_DIR}"

echo "==> 上传压缩包 ..."
scp "${ARCHIVE}" "${USER}@${HOST}:${REMOTE_DIR}/"

# ---- 远程解压 ----
echo "==> 远程解压并设置权限 ..."
ssh "${USER}@${HOST}" bash -s << REMOTE_EOF
cd ${REMOTE_DIR}
tar xzf ${ARCHIVE_NAME}
# 将二进制移到 tools 根目录，方便直接运行
EXTRACTED_DIR=\$(tar tzf ${ARCHIVE_NAME} | head -1 | cut -d/ -f1)
cp "\${EXTRACTED_DIR}/task-graph-editor" ./task-graph-editor
chmod +x ./task-graph-editor
rm -f ${ARCHIVE_NAME}
echo "==> 部署完成: ${REMOTE_DIR}/task-graph-editor"
REMOTE_EOF

echo ""
echo "==> 部署成功!"
echo "    远程路径: ${REMOTE_DIR}/task-graph-editor"
echo "    运行命令: ssh ${USER}@${HOST} '${REMOTE_DIR}/task-graph-editor'"

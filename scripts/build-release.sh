#!/usr/bin/env bash
# 打包部署脚本 - 构建 release 二进制并打包为可分发的压缩包
#
# 用法:
#   ./scripts/build-release.sh              # 构建本机平台
#   ./scripts/build-release.sh --target x86_64-unknown-linux-gnu  # 交叉编译
#
# 输出: dist/task-graph-editor-<version>-<target>.tar.gz

set -euo pipefail

# ---- 检测 Shell 环境 ----
if [ -n "${BASH_VERSION:-}" ]; then
    SHELL_TYPE=bash
elif [ -n "${ZSH_VERSION:-}" ]; then
    SHELL_TYPE=zsh
fi

# ---- 项目根目录 ----
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${PROJECT_DIR}"

# ---- 解析参数 ----
TARGET=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            TARGET="$2"
            shift 2
            ;;
        *)
            echo "未知参数: $1"
            echo "用法: $0 [--target <triple>]"
            exit 1
            ;;
    esac
done

# ---- 提取版本号 ----
VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
echo "==> 版本: ${VERSION}"

# ---- 质量闸门 ----
echo "==> 运行 cargo fmt --check ..."
cargo fmt --all -- --check

echo "==> 运行 cargo clippy ..."
cargo clippy --all -- -D warnings

echo "==> 运行 cargo test ..."
cargo test

# ---- 构建 Release ----
BUILD_ARGS=(--release)
if [ -n "${TARGET}" ]; then
    BUILD_ARGS+=(--target "${TARGET}")
    BINARY_DIR="target/${TARGET}/release"
else
    TARGET=$(rustc -vV | grep '^host:' | awk '{print $2}')
    BINARY_DIR="target/release"
fi

echo "==> 构建 release (target: ${TARGET}) ..."
cargo build "${BUILD_ARGS[@]}"

# ---- 打包 ----
DIST_DIR="${PROJECT_DIR}/dist"
PACKAGE_NAME="task-graph-editor-${VERSION}-${TARGET}"
PACKAGE_DIR="${DIST_DIR}/${PACKAGE_NAME}"

rm -rf "${PACKAGE_DIR}"
mkdir -p "${PACKAGE_DIR}"

# 复制二进制
BINARY="${BINARY_DIR}/task-graph-editor"
if [ ! -f "${BINARY}" ]; then
    echo "错误: 找不到构建产物 ${BINARY}"
    exit 1
fi
cp "${BINARY}" "${PACKAGE_DIR}/"

# 复制必要文件
[ -f README.md ] && cp README.md "${PACKAGE_DIR}/"
[ -f LICENSE ] && cp LICENSE "${PACKAGE_DIR}/"

# 生成运行说明
cat > "${PACKAGE_DIR}/README-DEPLOY.md" << 'DEPLOY_EOF'
# 任务图编辑器 - 部署说明

## 系统依赖

```bash
# Debian/Ubuntu
sudo apt install libssl3 libgl1-mesa-glx libxkbcommon0

# 如果使用 Wayland
sudo apt install libwayland-client0
```

## 运行

```bash
chmod +x task-graph-editor
./task-graph-editor
```

## 使用方式

1. 在左侧面板输入远程主机地址、端口(默认22)、用户名(默认linux)、密码
2. 点击"连接"按钮连接远程主机
3. 文件列表会自动加载 /home/linux/Workspace/task_graphs/ 下的 JSON 文件
4. 点击文件名加载并编辑 map_id、task_id 和所有位姿点
5. 修改完成后点击"更新远程文件"保存
DEPLOY_EOF

# 创建压缩包
echo "==> 打包 ${PACKAGE_NAME}.tar.gz ..."
cd "${DIST_DIR}"
tar czf "${PACKAGE_NAME}.tar.gz" "${PACKAGE_NAME}"
rm -rf "${PACKAGE_NAME}"

ARCHIVE="${DIST_DIR}/${PACKAGE_NAME}.tar.gz"
SIZE=$(du -h "${ARCHIVE}" | awk '{print $1}')
echo ""
echo "==> 构建完成!"
echo "    产物: ${ARCHIVE}"
echo "    大小: ${SIZE}"
echo "    版本: ${VERSION}"
echo "    平台: ${TARGET}"

#!/usr/bin/env bash
# 构建发布脚本 - 质量闸门 + Release 构建 + 多平台打包
#
# 用法:
#   ./scripts/build-release.sh              # 构建本机平台
#   ./scripts/build-release.sh --target x86_64-unknown-linux-gnu  # 交叉编译
#
# macOS 输出: dist/task-graph-editor-v<version>-macos-arm64.zip  (含 .app bundle)
# Linux 输出: dist/task-graph-editor-v<version>-linux-x64.tar.gz
# Windows 输出: dist/task-graph-editor-v<version>-windows-x64.zip

set -euo pipefail

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# 项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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
            echo -e "${RED}未知参数: $1${NC}"
            echo "用法: $0 [--target <triple>]"
            exit 1
            ;;
    esac
done

# ---- 提取版本号 ----
VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
if [ -z "$VERSION" ]; then
    echo -e "${RED}错误: 无法从 Cargo.toml 提取版本号${NC}"
    exit 1
fi
echo -e "${CYAN}=== 任务图编辑器 Release 构建 ===${NC}"
echo -e "${GREEN}版本: v${VERSION}${NC}"

# ---- 质量闸门 ----
echo -e "\n${YELLOW}[1/5] cargo fmt --check ...${NC}"
cargo fmt --all -- --check

echo -e "\n${YELLOW}[2/5] cargo clippy ...${NC}"
cargo clippy --all --all-targets -- -D warnings

echo -e "\n${YELLOW}[3/5] cargo test ...${NC}"
cargo test --all

# ---- 构建 Release ----
BUILD_ARGS=(--release)
if [ -n "${TARGET}" ]; then
    BUILD_ARGS+=(--target "${TARGET}")
    BINARY_DIR="target/${TARGET}/release"
else
    TARGET=$(rustc -vV | grep '^host:' | awk '{print $2}')
    BINARY_DIR="target/release"
fi

echo -e "\n${YELLOW}[4/5] cargo build --release (target: ${TARGET}) ...${NC}"
cargo build "${BUILD_ARGS[@]}"

# 检测平台名称
case "${TARGET}" in
    x86_64-unknown-linux-gnu)   PLATFORM="linux-x64" ;;
    aarch64-unknown-linux-gnu)  PLATFORM="linux-arm64" ;;
    x86_64-pc-windows-msvc)     PLATFORM="windows-x64" ;;
    aarch64-apple-darwin)       PLATFORM="macos-arm64" ;;
    x86_64-apple-darwin)        PLATFORM="macos-x64" ;;
    *)                          PLATFORM="${TARGET}" ;;
esac

# 可执行文件名
EXE_NAME="task-graph-editor"
if [[ "${TARGET}" == *"windows"* ]]; then
    EXE_NAME="task-graph-editor.exe"
fi

BINARY="${BINARY_DIR}/${EXE_NAME}"
if [ ! -f "${BINARY}" ]; then
    echo -e "${RED}错误: 找不到构建产物 ${BINARY}${NC}"
    exit 1
fi

# ---- 打包 ----
echo -e "\n${YELLOW}[5/5] 打包产物 ...${NC}"

DIST_DIR="${PROJECT_DIR}/dist"
PACKAGE_NAME="task-graph-editor-v${VERSION}-${PLATFORM}"
rm -rf "${DIST_DIR}/${PACKAGE_NAME}"* 2>/dev/null || true
mkdir -p "${DIST_DIR}"

if [[ "${TARGET}" == *"-apple-darwin"* ]]; then
    # macOS: .app bundle + zip
    APP_NAME="任务图编辑器.app"
    APP_DIR="${DIST_DIR}/${APP_NAME}"
    rm -rf "${APP_DIR}"
    mkdir -p "${APP_DIR}/Contents/MacOS"
    mkdir -p "${APP_DIR}/Contents/Resources"

    cp "${BINARY}" "${APP_DIR}/Contents/MacOS/task-graph-editor"

    cat > "${APP_DIR}/Contents/Info.plist" << PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>任务图编辑器</string>
    <key>CFBundleDisplayName</key>
    <string>任务图编辑器</string>
    <key>CFBundleIdentifier</key>
    <string>com.task-graph-editor</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleExecutable</key>
    <string>task-graph-editor</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
</dict>
</plist>
PLIST_EOF

    cd "${DIST_DIR}"
    zip -r -q "${PACKAGE_NAME}.zip" "${APP_NAME}"
    rm -rf "${APP_NAME}"
    ARCHIVE="${DIST_DIR}/${PACKAGE_NAME}.zip"
else
    # Linux / Windows: 裸二进制 + VERSION
    PACKAGE_DIR="${DIST_DIR}/${PACKAGE_NAME}"
    mkdir -p "${PACKAGE_DIR}"
    cp "${BINARY}" "${PACKAGE_DIR}/"
    echo -n "v${VERSION}" > "${PACKAGE_DIR}/VERSION"

    cd "${DIST_DIR}"
    if [[ "${TARGET}" == *"windows"* ]]; then
        # Windows: zip
        zip -r -q "${PACKAGE_NAME}.zip" "${PACKAGE_NAME}"
        rm -rf "${PACKAGE_NAME}"
        ARCHIVE="${DIST_DIR}/${PACKAGE_NAME}.zip"
    else
        # Linux: tar.gz
        tar czf "${PACKAGE_NAME}.tar.gz" "${PACKAGE_NAME}"
        rm -rf "${PACKAGE_NAME}"
        ARCHIVE="${DIST_DIR}/${PACKAGE_NAME}.tar.gz"
    fi
fi

SIZE=$(du -h "${ARCHIVE}" | awk '{print $1}')
echo -e "\n${CYAN}=== 构建完成 ===${NC}"
echo -e "${GREEN}产物: ${ARCHIVE}${NC}"
echo -e "${GREEN}大小: ${SIZE}${NC}"
echo -e "${GREEN}版本: v${VERSION}${NC}"
echo -e "${GREEN}平台: ${PLATFORM}${NC}"

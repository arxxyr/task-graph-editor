#!/usr/bin/env bash
# Bash 部署脚本 - 将编译产物收集到 bin 目录并打包
# 用法: ./scripts/deploy.sh [release|debug]
#
# macOS 自动生成 .app bundle（双击不弹终端）
# Linux/Windows 打包裸二进制 + VERSION

set -euo pipefail

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
GRAY='\033[0;37m'
NC='\033[0m'

# 默认编译配置
PROFILE="${1:-release}"

echo -e "${CYAN}=== 任务图编辑器 部署脚本 ===${NC}"
echo -e "${GREEN}编译配置: $PROFILE${NC}"

# 项目根目录
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 从 Cargo.toml 提取版本号
VERSION=$(grep '^version' "$ROOT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
if [ -z "$VERSION" ]; then
    echo -e "${RED}错误: 无法从 Cargo.toml 提取版本号${NC}"
    exit 1
fi
echo -e "${GREEN}版本号: v$VERSION${NC}"

cd "$ROOT_DIR"

# 目标目录
BIN_DIR="$ROOT_DIR/bin"

# 源目录
TARGET_DIR="$ROOT_DIR/target/$PROFILE"

# 检测平台
EXE_NAME="task-graph-editor"
IS_MACOS=false
IS_WINDOWS=false
if [[ "$OSTYPE" == "darwin"* ]]; then
    IS_MACOS=true
elif [[ "$OSTYPE" == "msys" || "$OSTYPE" == "cygwin" ]]; then
    IS_WINDOWS=true
    EXE_NAME="task-graph-editor.exe"
fi

echo -e "\n${YELLOW}[1/3] 清理旧的 bin 目录...${NC}"
if [ -d "$BIN_DIR" ]; then
    rm -rf "$BIN_DIR"
    echo -e "${GRAY}已删除旧目录: $BIN_DIR${NC}"
fi

echo -e "\n${YELLOW}[2/3] 复制可执行文件...${NC}"
mkdir -p "$BIN_DIR"
EXE_PATH="$TARGET_DIR/$EXE_NAME"
if [ -f "$EXE_PATH" ]; then
    cp "$EXE_PATH" "$BIN_DIR/"
    EXE_SIZE=$(du -h "$EXE_PATH" | cut -f1)
    echo -e "${GREEN}已复制: $EXE_NAME ($EXE_SIZE)${NC}"
else
    echo -e "${RED}错误: 找不到可执行文件 $EXE_PATH${NC}"
    echo -e "${YELLOW}请先运行: cargo build --${PROFILE}${NC}"
    exit 1
fi

echo -e "\n${YELLOW}[3/3] 创建版本压缩包...${NC}"

if $IS_MACOS; then
    # macOS: 生成 .app bundle
    APP_NAME="任务图编辑器.app"
    APP_DIR="$BIN_DIR/$APP_NAME"
    mkdir -p "$APP_DIR/Contents/MacOS"
    mkdir -p "$APP_DIR/Contents/Resources"
    cp "$BIN_DIR/$EXE_NAME" "$APP_DIR/Contents/MacOS/task-graph-editor"

    cat > "$APP_DIR/Contents/Info.plist" << PLIST_EOF
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

    ZIP_NAME="task-graph-editor-v${VERSION}.zip"
    cd "$BIN_DIR"
    rm -f task-graph-editor-v*.zip
    zip -r -q "$ZIP_NAME" "$APP_NAME"
    cd "$ROOT_DIR"
    echo -e "${GREEN}已创建: bin/$ZIP_NAME${NC}"

    echo -e "\n${CYAN}=== 部署完成 ===${NC}"
    echo -e "\n${GREEN}目录结构:${NC}"
    echo "bin/"
    echo "├── $EXE_NAME"
    echo "├── $ZIP_NAME"
    echo "└── 任务图编辑器.app/"
    echo "    └── Contents/"
    echo "        ├── Info.plist"
    echo "        ├── MacOS/task-graph-editor"
    echo "        └── Resources/"
    echo ""
    echo -e "${GREEN}运行方式:${NC}"
    echo "  open bin/任务图编辑器.app"
    echo ""
    echo -e "${GREEN}分发方式:${NC}"
    echo "  将 bin/$ZIP_NAME 发送给用户"
else
    # Linux / Windows: 裸二进制 + VERSION
    echo -n "v${VERSION}" > "$BIN_DIR/VERSION"
    ZIP_NAME="task-graph-editor-v${VERSION}.zip"
    cd "$BIN_DIR"
    rm -f task-graph-editor-v*.zip
    zip -r "$ZIP_NAME" . -x "*.zip"
    cd "$ROOT_DIR"
    echo -e "${GREEN}已创建: bin/$ZIP_NAME${NC}"

    echo -e "\n${CYAN}=== 部署完成 ===${NC}"
    echo -e "\n${GREEN}目录结构:${NC}"
    echo "bin/"
    echo "├── $EXE_NAME"
    echo "├── VERSION"
    echo "└── $ZIP_NAME"
    echo ""
    echo -e "${GREEN}运行方式:${NC}"
    echo "  cd bin"
    if $IS_WINDOWS; then
        echo "  ./task-graph-editor.exe"
    else
        echo "  ./task-graph-editor"
    fi
    echo ""
    echo -e "${GREEN}分发方式:${NC}"
    echo "  将 bin/$ZIP_NAME 发送给用户"
fi
echo ""

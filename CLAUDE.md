# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

任务图编辑器（task-graph-editor）—— 基于 eframe/egui 的跨平台桌面 GUI 应用，通过 SSH 连接远程 Linux 主机，编辑机器人任务图 JSON 文件中的位姿数据，并支持从 ROS2 话题实时获取底盘位姿和关节角。

## 项目结构

```
task-graph-editor/
├── Cargo.toml                  # 项目配置（Rust 2024 edition）
├── CLAUDE.md                   # 开发指南
├── .github/workflows/build.yml # GitHub Actions CI
├── .gitlab-ci.yml              # GitLab CI
├── src/
│   ├── main.rs                 # 入口：mimalloc、字体嵌入、窗口配置
│   ├── model.rs                # 数据模型：位姿结构体、JSON 解析/序列化、ROS2 输出解析、登录持久化
│   ├── app.rs                  # GUI 应用：连接面板、文件列表（右键菜单：上传/备份/删除）、元数据编辑、位姿编辑器、ROS2 数据获取
│   └── ssh.rs                  # SSH/SFTP 封装：连接、认证、文件操作、命令执行
├── assets/fonts/               # 更纱黑体（SarasaTermSCNerd，编译时嵌入）
└── scripts/
    ├── build-release.sh        # 本地 Release 构建脚本
    └── deploy-remote.sh        # 远程部署脚本
```

## 构建与开发命令

```bash
# 构建
cargo build
cargo build --release

# 运行
cargo run

# 测试
cargo test
cargo test <test_name>           # 运行单个测试
cargo test -- --nocapture        # 显示测试中的 println 输出

# 代码检查（提交前必须通过）
cargo fmt --all
cargo clippy --all --all-targets

# 检查编译但不生成产物
cargo check

# 本地 Release 构建（含 fmt/clippy/test 闸门）
./scripts/build-release.sh

# 远程部署
./scripts/deploy-remote.sh <host> [user] [remote_dir]
```

## 架构与数据流

```
egui UI (app.rs)
    ↕
TaskGraphData (model.rs)          LoginConfig → ~/.config/task-graph-editor/login.json
    ↕
JSON 字符串 ←→ SshConnection (ssh.rs) ←→ 远程文件 /home/linux/Workspace/task_graphs/{task_id}.json
                    ↕
              ROS2 命令（ros2 topic echo / python3 脚本）
```

- `raw_json` 保留未编辑的原始 JSON，序列化时仅更新编辑过的字段
- JSON `config.context` 中的位姿字段是**字符串化 JSON**，需要二次解析
- 修改 `task_id` 后远程文件自动重命名为 `{task_id}.json`

## 关键依赖

| 依赖 | 用途 |
|------|------|
| `eframe` 0.31 | egui 桌面应用框架 |
| `ssh2` 0.9 | SSH/SFTP 协议（同步） |
| `serde` + `serde_json` | JSON 序列化/反序列化 |
| `thiserror` 2 | 错误类型派生 |
| `mimalloc` 0.1 | 全局内存分配器 |
| `rfd` 0.15 | 跨平台文件对话框 |
| `chrono` 0.4 | 本地时间格式化（备份文件名时间戳） |

## 约定

- **语言**：只用中文交流与注释
- **命名**：函数/变量 `snake_case`，类型 `UpperCamelCase`
- **Rust edition**：2024
- **全局分配器**：mimalloc（`#[global_allocator]`）
- **错误处理**：优先 `Result`/`Option`，避免 `unwrap()` 在非测试代码中使用
- **无异步运行时**：全同步 SSH/SFTP，无 tokio 依赖
- **字体**：更纱黑体通过 `include_bytes!` 编译时嵌入，零运行时依赖
- **Windows**：Release 构建隐藏控制台窗口（`windows_subsystem = "windows"`）

## CI 流水线

三阶段串行（GitHub Actions + GitLab CI）：

1. **Lint**（并行）：`cargo fmt --check` + `cargo clippy -D warnings`
2. **Test**：`cargo test --all`
3. **Build**（三平台并行）：Linux x64 / macOS ARM64 / Windows x64

- 工具链：Rust nightly
- 缓存：Swatinem/rust-cache@v2
- UPX 压缩：Linux/Windows `--best --lzma`，macOS 跳过
- 产物命名：`task-graph-editor-{版本}-{平台}.{扩展名}`
- 推送 `v*` 标签自动创建 Release（含 prerelease 检测）

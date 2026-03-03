# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

任务图编辑器（task-graph-editor）—— Rust 2024 edition 项目。

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
cargo clippy --all

# 检查编译但不生成产物
cargo check
```

## 约定

- **语言**：只用中文交流与注释
- **命名**：函数/变量 `snake_case`，类型 `UpperCamelCase`
- **Rust edition**：2024
- **全局分配器**：使用 mimalloc（添加依赖后配置 `#[global_allocator]`）
- **错误处理**：优先返回值（`Result`/`Option`），避免 `unwrap()` 在非测试代码中使用
- **异步**：若用 Tokio，注意不要在 async 中做同步 I/O，不要持锁 await
- **序列化**：高频路径避免 JSON，考虑 bincode；用 `&str` 代替 `String` 减少拷贝

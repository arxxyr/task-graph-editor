# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

任务图编辑器（task-graph-editor）—— 基于 **Bevy 0.19 + BSN + Feathers** 的跨平台桌面 GUI 应用，通过 SSH 连接远程 Linux 主机，编辑机器人任务图 JSON 文件中的所有 context 全局变量（位姿、轨迹、数组、标量、布尔等），并支持从 ROS2 话题实时获取底盘位姿和关节角。

## 项目结构

```
task-graph-editor/
├─ Cargo.toml                  # 项目配置（Rust 2024 edition）
├─ AGENTS.md                   # 开发指南（CLAUDE.md 为指向它的符号链接）
├─ .github/workflows/build.yml # GitHub Actions CI
├─ .gitlab-ci.yml              # GitLab CI
├─ src/
│  ├─ main.rs                  # 入口：mimalloc、Bevy App 装配、窗口与日志配置
│  ├─ model.rs                 # 数据模型：ContextValue（12 种变体）、JSON 解析/序列化、ROS2 输出解析、登录持久化
│  ├─ ssh.rs                   # SSH/SFTP 封装：连接、认证（密码 / ssh-agent / 私钥）、文件操作、命令执行
│  ├─ ssh_config.rs            # ~/.ssh/config 解析：Host/Match/Include、首值生效、token 展开
│  ├─ worker.rs                # 后台工作线程：所有 SSH/SFTP/ROS2 操作在此异步执行（与 GUI 框架解耦）
│  └─ ui/
│     ├─ mod.rs                # 状态资源（Session/StatusLine/FileBrowser/Editor）、插件装配
│     ├─ theme.rs              # 主题：OKLCH 配色、应用自定义 token、尺寸常量
│     ├─ fonts.rs              # 中文字体嵌入与全局覆盖
│     ├─ widgets.rs            # 通用构件：卡片、折叠区块、表单行、数值/文本/密码输入
│     ├─ password.rs           # 密码遮罩控件（Bevy 未提供，基于光标位置同步）
│     ├─ binding.rs            # 数值控件 ↔ TaskGraphData 的绑定与回写（纯逻辑，带单测）
│     ├─ shell.rs              # 界面骨架：顶栏 / 侧栏 / 内容区 / 状态栏 / 右键菜单层
│     ├─ connect.rs            # 连接面板：SSH 表单、ssh config 主机下拉、连接按钮组
│     ├─ files.rs              # 文件列表：选中、右键菜单（上传/备份/删除）
│     ├─ editor.rs             # 字段树编辑器：六个分组、递归嵌套、懒加载
│     └─ screenshot.rs         # 截图：F12 手动 / TGE_SCREENSHOT 自动（CI 视觉回归）
├─ assets/fonts/               # 更纱黑体（SarasaTermSCNerd，编译时嵌入）
└─ scripts/
   ├─ build-release.sh         # 本地 Release 构建脚本
   └─ deploy-remote.sh         # 远程部署脚本
```

## 构建与开发命令

```bash
# 构建
cargo build
cargo build --release

# 运行
cargo run

# 截图（不需要屏幕录制权限，由 Bevy 自己渲染后落盘）
TGE_SCREENSHOT=/tmp/ui.png cargo run     # 启动后自动截一张再退出
# 运行时按 F12 也可随时截图到当前目录

# 排查问题：打开详细日志（SSH 连接参数、SFTP 路径展开、文件列表条数、界面重建时机）
TGE_LOG=debug cargo run
TGE_LOG=trace cargo run                  # 再加上 wgpu/naga 的警告

# 测试
cargo test
cargo test <test_name>           # 运行单个测试
cargo test -- --nocapture        # 显示测试中的 println 输出

# 代码检查（提交前必须通过）
cargo fmt --all
cargo clippy --all --all-targets -- -D warnings

# 本地 Release 构建（含 fmt/clippy/test 闸门）
./scripts/build-release.sh

# 远程部署
./scripts/deploy-remote.sh <host> [user] [remote_dir]
```

## 架构与数据流

```
BSN 场景 (ui/*.rs) ──AppAction 消息──→ worker_bridge ──WorkerRequest──→ Worker 线程 (worker.rs)
    ↑                                       ↑                                ↓
    └──状态资源变更驱动重建───────────────────┴──WorkerResponse──── SshConnection (ssh.rs)
                                                                        ↕
TaskGraphData (model.rs)          LoginConfig → ~/.config/task-graph-editor/login.json    远程文件 + ROS2 命令
```

### 数据模型（与 GUI 框架无关，重构时保持不变）

- `raw_json` 保留未编辑的原始 JSON，序列化时仅更新编辑过的字段
- JSON `config.context` 中的位姿字段是**字符串化 JSON**，需要二次解析
- `config.context` 中所有字段自动识别类型（`ContextValue` 枚举，12 种变体）：
  - `Pose` — 字符串化 RobotPose（chassis/head/waist）
  - `Bool` / `Integer` / `Float` — 标量
  - `NumericArray` / `NumericArray2D` — 1D/2D 数值数组。真实任务图里两种写法都有
    （字符串化的 `"[0.01,0.17]"` 与原生 JSON 数组），`stringified` 记住原样，
    序列化按原形式写回，否则会悄悄改掉远程文件的数据格式
  - `JointTrajectory` — 原生 JSON 数组（positions + time_from_start）
  - `PoseArray` — 原生位姿数组
  - `NestedGroup` — 原生 JSON 对象，成员递归分类（如 `station_profiles.station_1.*`），支持任意层级嵌套
  - `Text` / `Null` / `RawJson` — 其他
- 字符串化数组里的整数保持整数格式（如 `"[4,3]"` 不会变成 `"[4.0,3.0]"`）；
  原生数组则照 f64 写回，`0.0` 不能收敛成 `0`——那会把 JSON 类型从 float 改成 int
- **位姿键顺序统一为 position 在前**：真实文件里两种顺序都有（机器人端 Python 写的是
  字母序，orientation 在前），本工具一律按 `Pose` 的字段声明顺序写出
- `serde_json` 开了 `float_roundtrip`：默认的快速浮点解析有 1 ULP 误差
  （实测 `0.9216510910864573` 会被读成 `...572`），位姿坐标读一遍存回去就变了
- 解析覆盖率可用 `TGE_ANALYZE=<文件> cargo test 分析 -- --ignored --nocapture` 检查，
  会列出未能识别的字段并输出往返结果供比对
- 修改 `task_id` 后远程文件自动重命名为 `{task_id}.json`

### UI 层：保留模式下的重建策略

Bevy 是保留模式 UI，控件是实体而非每帧重绘的立即模式绘制。BSN（0.19）是**代码驱动、无响应式**的，
所以状态变化后要显式决定重建什么。核心规则：

| 变化 | 处理 | 原因 |
|------|------|------|
| 加载文件、null 改成位姿 | `Editor::structure_version` 递增 → 重建整棵字段树 | 字段树形状变了 |
| 输入框里改数值 | **不重建**，由 observer 按绑定写回数据 | 重建会让输入焦点每敲一个字符就丢失 |
| ROS2 回填位姿 | `Editor::value_version` 递增 → 只推新值给已有控件 | 数据被 UI 之外的来源改动 |
| 选中位姿 | 只换 `ThemeBackgroundColor` / `ThemeTextColor` | 高亮不涉及结构 |
| 文件列表变化 | `FileBrowser::list_version` 递增 → 重建行 | 行数变了 |
| 文件选中变化 | 只加减 `Selected` 组件 | 避免重建打断滚动位置 |

- 每个重建 system 都用「已渲染版本号」做守卫（`RenderedEditor` / `RenderedList` / `RenderedVersions`），
  避免每帧重复重建。**已渲染版本一律用 `Option<u64>`，不要拿 0 当"还没建过"的哨兵** ——
  版本号本身从 0 起算，用 0 当哨兵会让第一次真实更新（0 → 1）被误判成已渲染而整个丢掉
  （表现就是"连上了但文件列表刷不出来"、"点了文件但编辑器是空的"）
- `queue_spawn_related_scenes` 是**跨帧**的：排队后要等 asset 依赖加载完才挂上 `Children`。
  由此引出两条铁律：
  - 判断"是否已填充"不能只看 `Children` 是否为空，要用标记组件（见 `editor::LazyFilled`）
  - **`despawn_related` 删不掉排队中的那批**。上一批还没落地时又提交一批，两批都会挂上去，
    表现就是按钮、列表项凭空多出一份。所有插槽重建统一走 `widgets::replace_slot_children`，
    配合 `SlotPending` 保证同一时刻只有一批在飞；跳过时**不要**推进已渲染版本号，下一帧会自动重试
- **属性变化不要当重建条件**。按钮的禁用态、选中高亮这类是属性，改组件即可
  （`widgets::ButtonGate` → `InteractionDisabled`）。把它们塞进重建条件会让状态一翻转就重建整块，
  正好撞上上面那个竞态——"点一下刷新列表冒出六个按钮"就是这么来的
- 轨迹点、二维数组行的内容**懒加载**：展开时才建控件，避免大轨迹一次生成上千个输入框

### 数值绑定

数值控件挂 `ValueBinding { field_path, slot }`，描述"我编辑的是数据里的哪一个数"：
`field_path` 是索引路径（首元素索引顶层 `context_fields`，后续依次下钻嵌套分组），
`slot` 指明字段内部位置（位姿分量、数组下标、轨迹点关节等）。
控件发出 `ValueChange` 时由统一的 observer 按绑定回写。这部分是纯数据寻址，`binding.rs` 里有完整单测。

**数值一律走 f64**（`NumberFormat::F64`）、整数走 i64。ROS2 关节角是 9 位小数，f32 会丢精度。

### 界面结构

```
顶栏    ● 任务图编辑器  当前文件           [应用到远程文件] [获取底盘位姿] …
侧栏    SSH 连接卡片（表单 + ssh config 下拉 + 按钮组）
        文件列表卡片（单击加载，右键上传/备份/删除）
内容区  元数据卡片（map_id / task_id）
        Context 参数卡片 → 六个分组：位姿点位 / 基本参数 / 数组参数 / 轨迹数据 / 嵌套分组 / 其他
状态栏  ● 状态消息 / 忙碌 / 重连（三选一）                    user@host:port
```

- 状态栏左侧永远只显示一条，优先级 重连 > 忙碌 > 最近一条结果——三者是同一件事的不同阶段，
  各占一边会变成「左下角和右下角都在说正在加载」；右侧放连接目标
- 位姿卡片点击选中（琥珀色高亮），选中后才能用顶栏的三个 ROS2 取数按钮
- 位姿的位置 xyz、姿态 wxyz 各横排一行，色条区分轴向（X 红 / Y 绿 / Z 蓝）
- UI 缩放：Shift + `+`/`-` 或 Shift + 滚轮（`UiScale`，0.5～3.0）
- 连接面板的主机框带下拉菜单，数据来自本机 `~/.ssh/config`（每次打开菜单重新解析，也可手动输入）；
  选中后填入 HostName/Port/User/IdentityFile，`ProxyJump`/`ProxyCommand` 条目会标注"跳板 · 不支持，将直连"
- 认证规则：密码非空 → 密码认证；密码为空 → 公钥认证（ssh-agent 全部身份 → 私钥框指定文件 →
  `~/.ssh/id_rsa`/`id_ecdsa`/`id_ed25519` 中存在者），全部失败时汇总每一步原因

### 远程路径里的 `~`

SFTP 协议**不做 shell 展开**：`~/Workspace/task_graphs` 会被服务端按相对路径解析成
`<home>/~/Workspace/task_graphs`，直接报"没有那个文件或目录"。而在"远程目录"里填 `~/...`
是很自然的写法，所以连接后会 `printf %s "$HOME"` 取一次远程 home 缓存起来，
`SshConnection` 的每个路径入口都过 `resolve_path` 展开（`ssh.rs::expand_home`，带单测）。

同理，删除文件走 SFTP `unlink` 而不是 `rm -f '<path>'`——后者把路径放进单引号里，
shell 同样不会展开其中的 `~`。

## 关键依赖

| 依赖 | 用途 |
|------|------|
| `bevy` 0.19 | 应用框架、ECS、UI、BSN 场景（按需挑 feature，不含 3D/音频/手柄） |
| `bevy_feathers` | 控件集：按钮、复选框、文本/数值输入、菜单、列表行、面板容器 |
| `ssh2` 0.9 | SSH/SFTP 协议（同步） |
| `serde` + `serde_json` | JSON 序列化/反序列化 |
| `thiserror` 2 | 错误类型派生 |
| `mimalloc` 0.1 | 全局内存分配器 |
| `rfd` 0.17 | 跨平台文件对话框 |
| `chrono` 0.4 | 本地时间格式化（备份文件名、截图文件名） |
| `icu_provider` | **不直接使用**，只为开启其 `logging` feature，见下 |

### 两个绕不开的上游限制

1. **CJK 断行词典缺失**：parley 用 `new_for_non_complex_scripts` 构造分段器，不带 CJK 词典，
   中文文本每次布局都会警告一次。而 `icu_provider` 在 `logging` feature 关闭时把 `log::warn!`
   别名成 `std::eprintln!`（只在 debug 构建），直接写 stderr 绕过日志框架 —— debug 下会刷屏。
   解法：在 `Cargo.toml` 显式依赖 `icu_provider` 并开启 `logging`，让它归入 log 门面，
   再由 `main.rs` 里 `LogPlugin` 的 `icu_provider=off` 统一屏蔽。断行退化为按字形断，短标签无影响。

2. **密码遮罩**：Bevy 0.19 的 `EditableText` 明确未实现密码遮罩。`ui/password.rs` 自建一层：
   真实密码存组件里，文本框只放等长遮罩。**定位编辑点靠光标位置而非内容比对** ——
   遮罩串每个字符都一样，删掉中间任意一个结果完全相同，前后缀匹配会一律误判成"删末尾"。

## 约定

- **语言**：只用中文交流与注释
- **命名**：函数/变量 `snake_case`，类型 `UpperCamelCase`
- **Rust edition**：2024
- **全局分配器**：mimalloc（`#[global_allocator]`）
- **错误处理**：优先 `Result`/`Option`，避免 `unwrap()` 在非测试代码中使用
- **控制流**：多分支优先 `match`，避免 if-else 链
- **原生对话框必须钉在主线程**：`rfd` 弹的是 `NSOpenPanel`，它在非主线程会
  `dispatch_sync` 到主队列；而 Bevy 的多线程调度器此刻正在主线程 `block_on` 等这个
  system 跑完——两边互等，直接死锁（进程不崩、永久挂起，连崩溃报告都没有）。
  给 system 加 `NonSendMarker` 参数即可钉住主线程，见 `worker_bridge::handle_file_dialog`。
  Bevy 会把普通 system 丢到 Compute Task Pool，**不要默认自己在主线程**
- **异步模式**：SSH 操作在后台线程执行（`std::thread` + `mpsc`），无 tokio 依赖。
  后台线程通过 winit 的 `EventLoopProxy` 唤醒主循环（`worker::WakeFn`，与 GUI 框架解耦）
- **窗口刷新**：`WinitSettings` 用 reactive 模式，无输入时不重绘，空闲 CPU 占用接近零
- **字体**：更纱黑体通过 `include_bytes!` 编译时嵌入，零运行时依赖。
  Feathers 控件内部硬编码 FiraSans（无 CJK 字形），靠 `fonts.rs` 的 system 在布局前统一覆盖
- **主题**：颜色一律走 `ThemeToken`，不要在场景里写死 `Color`。
  `ThemeBackgroundColor` / `ThemeTextColor` 是不可变组件，改色要 `insert` 整个组件而非 `get_mut`
- **Windows**：Release 构建隐藏控制台窗口（`windows_subsystem = "windows"`）
- **profile**：依赖用 `opt-level = 3` 编译（dev 与 release 都是），Bevy 在低优化下交互明显发涩

## CI 流水线

三阶段串行（GitHub Actions + GitLab CI）：

1. **Lint**（并行）：`cargo fmt --check` + `cargo clippy -D warnings`
2. **Test**：`cargo test --all`
3. **Build**（三平台并行）：Linux x64 / macOS ARM64 / Windows x64

- 工具链：Rust nightly
- 缓存：Swatinem/rust-cache@v2
- Linux 系统依赖：`libxkbcommon-dev libgl1-mesa-dev libwayland-dev libx11-dev libxcursor-dev
  libxrandr-dev libxi-dev`（不需要 libasound2-dev / libudev-dev —— 音频与手柄 feature 都没开）
- UPX 压缩：仅 Linux `--best --lzma`；macOS 不支持；Windows 跳过（UPX 加壳的无签名 exe 会触发 Defender/SmartScreen 木马误报）
- 产物命名：`task-graph-editor-{版本}-{平台}.{扩展名}`
- 推送 `v*` 标签自动创建 Release（含 prerelease 检测）

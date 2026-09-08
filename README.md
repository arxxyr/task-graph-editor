# 任务图编辑器（Task Graph Editor）

基于 Rust + Bevy（BSN 场景 + Feathers 控件）的跨平台桌面应用，通过 SSH 远程编辑机器人任务图 JSON 文件中的 context 参数与执行流程。

## 功能

- **SSH 远程连接** — 支持密码和密钥认证，连接中可取消
- **异步操作** — 所有 SSH/SFTP 操作在后台线程执行，网络不稳定时 UI 不卡顿
- **任务图编辑** — 图形化编辑 `map_id`、`task_id` 及 context 中所有位姿字段
- **位姿编辑器** — 底盘（chassis）、头部（head）、腰部（waist）三部位的位置 + 四元数姿态编辑，保留完整 f64 精度
- **ROS2 数据获取** — 通过 SSH 远程执行 `ros2 topic echo` 获取底盘实时位姿，通过 Python 脚本获取关节角
- **文件管理** — 列表浏览、加载、保存、上传本地文件、备份、删除，修改 task_id 自动重命名远程文件
- **右键菜单** — 文件列表支持右键操作：上传文件、备份（时间戳命名，自动去重）、删除
- **登录持久化** — 连接信息保存到 `~/.config/task-graph-editor/login.json`
- **分组编辑器** — context 字段按类型分成位姿点位 / 基本参数 / 数组参数 / 轨迹数据 / 嵌套分组 / 其他六组，嵌套分组递归展开
- **流程图编辑** — 编辑完整节点与连线属性，增删、复制、改名、重连、设置根入口及维护复合子图；支持撤销和重做，保留未知字段与原始数据形式
- **节点目录** — 依据机器人执行器的 44 种已注册类型提供草稿和字段校验，业务参数由使用者填写；兼容原文件中的未知类型
- **画布操作** — 节点拖动、多选与框选、端口连线、独立缩放和平移、适配画布与恢复自动布局；浏览模式支持子图下钻、连线追踪和参数全文查看
- **流程诊断** — 异常节点与连线显示 JSON 路径和原始/可绘制条数，重复 ID 不会导致布局崩溃；创建 context 位姿保留下钻位置
- **保存保护** — 非法输入标红并阻止保存；切换文件、断开与关闭前保护未保存内容，保存期间的新修改继续标记为未保存；同目录原子提交，拒绝同名重命名覆盖
- **UI 缩放** — Shift + `+`/`-` 或 Shift + 鼠标滚轮调整界面缩放（0.5x ~ 3.0x）
- **四套主题** — 默认石墨青，另有暮砂金、暖纸橙、雾白靛；右上角选择后立即生效，并记住上次选择
- **中文界面** — 更纱黑体编译时嵌入，完整中文支持
- **低功耗** — 无输入时不重绘，空闲 CPU 占用接近零；后台线程有响应时主动唤醒
- **截图** — F12 随时截图，便于反馈界面问题
- **提示复制** — 左下角「复制提示」一键复制完整状态或错误信息，包括显示不下的长文本
- **远程目录支持 `~`** — 自动展开为远程 home 绝对路径（SFTP 本身不认 `~`）
- **排查日志** — `TGE_LOG=debug` 打开 SSH/SFTP/界面重建的详细日志

## 构建

### 前置依赖

**Debian/Ubuntu：**

```bash
sudo apt-get install -y \
    libssl-dev pkg-config \
    libxkbcommon-dev libgl1-mesa-dev libwayland-dev \
    libx11-dev libxcursor-dev libxrandr-dev libxi-dev
```

**macOS：**

```bash
brew install openssl pkg-config
```

**Windows：**

需要安装 [NASM](https://www.nasm.us/)（用于 OpenSSL vendored 编译）。

### 编译运行

```bash
# 安装 Rust nightly
rustup install nightly
rustup default nightly

# 开发构建
cargo run

# Release 构建
cargo build --release

# 本地 Release 构建（含代码检查和测试闸门）
./scripts/build-release.sh
```

Linux/macOS 运行测试还需要 `python3`，用于本地验证远端提交脚本。部署回归可单独运行：

```bash
python3 scripts/test_deploy_remote.py
```

### 远端要求

目标为 Linux，需要 SSH/SFTP、`python3`，SFTP 服务端须支持 `fsync@openssh.com`。
保存、备份和上传会先写临时文件并确认完整性，再原子发布；不满足要求会报错并保留原文件。
ROS2 取数另需远端的 ROS2 Humble 和机器人环境。

## 使用

1. 启动应用，在左侧面板填写 SSH 连接信息（主机、端口、用户名、密码）
2. 点击「连接」，认证成功后自动加载远程目录下的 JSON 文件列表
3. 点击文件名加载任务图，右侧显示元数据和按类型分组的 context 编辑器
4. 编辑数据，或点击位姿卡片选中后用顶栏按钮从 ROS2 获取实时数据
5. 右键文件可备份、删除，右键空白区域可上传本地文件
6. 点击「应用到远程文件」将修改写回远程文件

元数据卡片中的“保存位置”记录文档的实际来源。修改左侧目录后点击刷新可浏览另一目录，当前文档
仍保存到原位置；重新连接需重新加载文档。修改 `task_id` 会更名，目标同名文件已存在时拒绝覆盖。

应用不校验服务器主机指纹，也不读写 `known_hosts`；SSH 握手后直接进行密码或公钥认证。
网络在提交时中断可能丢失成功回执，按提示刷新核实。

### 编辑流程

切换到「流程图」并点击「编辑流程」。选中节点或连线后，右侧可编辑全文属性、布尔值和 JSON 数组或对象，
点击「应用修改」将草稿加入文档。「应用到远程文件」也会先应用当前合法草稿，再保存流程、context 和元数据。
非法草稿会保留输入和错误提示。撤销与重做只作用于流程修改，context 数值与实际远端文件名分别维护。

工具栏支持新增节点、连接及删除选中项；节点表单可复制、改名、设置根入口及进入或编辑子图。
`sequence`、`loop`、`parallel` 可清空子图并保留容器，`condition` 按执行器区分条件叶和条件容器。
`parallel` 的直接子节点并行执行，连线不会改变执行顺序；连续步骤应放入一个 `sequence` 分支。
节点目录及表达式规则依据见 [执行器协议](docs/graph-executor-protocol.md)，静态校验不代替机器人环境中的执行验证。

| 操作 | 方式 |
| --- | --- |
| 进入子图 | 双击带子图的节点，或点详情中的「进入子图」；浏览和编辑模式均支持 |
| 移动节点 | 编辑模式下拖动节点；多选后一起移动 |
| 多选与框选 | Shift 点击节点，或在画布空白处拖出选框 |
| 创建连接 | 从起点底部端口拖到终点顶部端口 |
| 修改连接端点 | 选中连线，点「重连起点」或「重连终点」，再点击新节点的底部或顶部端口；也可修改右侧 `from` / `to` 后点「应用修改」 |
| 缩放画布 | Ctrl/⌘ + 滚轮，范围 0.25～3.0 倍 |
| 平移画布 | 中键拖动，或按住空格拖动 |
| 适配与恢复位置 | 「适配画布」「自动布局」按钮 |
| 取消手势 | Esc |
| 撤销、重做与保存 | Ctrl/⌘ + Z、Shift + Z 或 Y、S；文本输入中的撤销保留给文本框 |

手工位置和缩放只属于当前本地视图，返回子图时恢复，重载文档时清空；不会写入机器人 JSON。
属性应用保留输入焦点和滚动位置；切换目标前先应用或重置尚未处理的草稿。

右上角「选择主题」可随时切换石墨青、暮砂金、暖纸橙、雾白靛，无需连接 SSH。
切换只刷新颜色，不重建现有控件，保留输入状态和滚动位置。
默认使用石墨青；实际切换后将选择保存到 `~/.config/task-graph-editor/theme.json`，下次启动自动恢复。
该文件与登录配置独立，启动不会主动写入；损坏文件保留原样。

## 项目结构

```
src/
├── main.rs        # 入口：mimalloc 分配器、Bevy App 装配、窗口与日志配置
├── model.rs       # 数据模型：位姿结构体、JSON 解析/序列化、登录持久化
├── model/         # 流程解析与诊断、图事务和稳定身份、执行器节点定义
├── ssh.rs         # SSH/SFTP：连接、文件操作、远程命令执行
├── ssh/           # agent 认证、原子保存与取消控制
├── ssh_config.rs  # ~/.ssh/config 解析（主机下拉菜单数据源）
├── worker.rs      # 后台工作线程：SSH/SFTP/ROS2 操作异步执行，mpsc 通信
└── ui/            # Bevy UI 层
    ├── theme.rs       # 四套主题、token 映射与切换
    ├── theme/
    │   ├── palette.rs # 内嵌色板解析与校验
    │   └── refresh.rs # 继承文字、光标与控件派生颜色刷新
    ├── theme_picker.rs # 右上角主题选择菜单
    ├── theme_preferences.rs # 独立主题偏好的加载与原子保存
    ├── fonts.rs       # 中文字体嵌入与覆盖
    ├── widgets.rs     # 卡片、折叠区块、表单行、各类输入控件
    ├── password.rs    # 密码遮罩控件
    ├── binding.rs     # 数值控件与数据的绑定回写
    ├── shell.rs       # 顶栏 / 侧栏 / 内容区 / 状态栏骨架
    ├── connect.rs     # 连接面板
    ├── files.rs       # 文件列表与右键菜单
    ├── editor.rs      # 字段树编辑器
    ├── graph_layout.rs # 流程图分层与避障走线
    ├── graph_view.rs   # 流程图视图、下钻与详情
    ├── graph_view/     # 画布拖动、端口、缩放与命中
    ├── graph_edit.rs   # 属性草稿、图操作与快捷键
    ├── graph_edit/     # 编辑表单与集成测试
    ├── document_state.rs # 已保存快照与保存回执身份
    ├── document_guard.rs # 切换、断开及关闭前的未保存保护
    ├── worker_bridge.rs # 动作处理、来源校验与后台响应
    └── screenshot.rs  # 截图
```

色板的唯一来源是 [assets/themes/palettes.json](assets/themes/palettes.json)，编译时嵌入，无需额外运行时资源。
设计说明和对比度数据见 [主题文档](docs/theme-proposals/README.md)。离线 UI 测试或截图夹具应注入
`ThemePreferencesStore::disabled()`，避免读取或修改使用者的主题配置。

## 发布

[CHANGELOG.md](CHANGELOG.md) 是版本变更记录的唯一来源：开发时把变化写入「未发布」，
发布前整理为 `## [版本号] - YYYY-MM-DD`，版本号与 `Cargo.toml` 保持一致，并填写实际发布日期。
每个版本只保留一个条目，使用「新增 / 修复 / 变更 / 移除 / 工程 / 验证」等三级标题组织内容。
更新日志从 `0.8.2` 开始记录，已发布条目不追加后续开发内容。链接使用完整 URL 的内联形式，
不引用条目外的链接定义，确保提取到 Release 后仍可打开。

GitHub Release 只显示标签对应的条目，并附版本信息和下载说明。标签与应用版本不一致，或
条目缺失、重复、日期无效、正文为空时，流水线会在编译前停止；发布前会再次检查并提取。
「未发布」和其他版本不会被放进当前 Release，也不会再追加自动生成的提交记录。

本地预览和回归检查（Python 3.11+，仅使用标准库）：

```bash
uv run --no-project python scripts/release_notes.py --tag v0.9.0
uv run --no-project python scripts/test_release_notes.py
```

GitHub Actions 仅在推送 `master` 时执行分支 CI；`dev` 等开发分支通过 PR 执行合并前检查，
同步推送 `master` 和 `dev` 不会重复触发两套分支构建。

推送 `v*` 标签时，只有标签指向的提交已包含在 GitHub 远端 `master` 的历史中，才允许构建并发布
Release。轻量标签、附注标签以及 `master` 历史提交上的标签均支持；未合入 `master` 的开发提交、
无法读取远端 `master` 或标签提交不一致时，门禁失败并停止后续任务。发布前还会重新校验一次。
标签本身不记录创建时所在分支，因此以提交是否已合入 `master` 为准。

以下使用本仓库的 GitHub 远端名 `github`，打标签前先同步并通过 `master` 的 CI：

```bash
git switch master
git tag v0.9.0
git push github v0.9.0
```

发布门禁的本地隔离测试：`python3 scripts/test_release_tag.py`，只创建临时 Git 仓库。

产物格式：

| 平台 | 文件名 |
|------|--------|
| Linux x64 | `task-graph-editor-v0.9.0+{commit}-linux-x64.tar.gz` |
| macOS ARM64 | `task-graph-editor-v0.9.0+{commit}-macos-arm64.zip`（应用包） |
| Windows x64 | `task-graph-editor-v0.9.0+{commit}-windows-x64.zip` |

## 许可证

MIT

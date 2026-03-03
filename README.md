# 任务图编辑器（Task Graph Editor）

基于 Rust + egui 的跨平台桌面应用，通过 SSH 远程编辑机器人任务图 JSON 文件中的位姿数据。

## 功能

- **SSH 远程连接** — 支持密码和密钥认证，SFTP 文件操作
- **异步操作** — 所有 SSH/SFTP 操作在后台线程执行，网络不稳定时 UI 不卡顿
- **任务图编辑** — 图形化编辑 `map_id`、`task_id` 及 context 中所有位姿字段
- **位姿编辑器** — 底盘（chassis）、头部（head）、腰部（waist）三部位的位置 + 四元数姿态编辑，保留完整 f64 精度
- **ROS2 数据获取** — 通过 SSH 远程执行 `ros2 topic echo` 获取底盘实时位姿，通过 Python 脚本获取关节角
- **文件管理** — 列表浏览、加载、保存、上传本地文件、备份、删除，修改 task_id 自动重命名远程文件
- **右键菜单** — 文件列表支持右键操作：上传文件、备份（时间戳命名，自动去重）、删除
- **登录持久化** — 连接信息保存到 `~/.config/task-graph-editor/login.json`
- **UI 缩放** — Shift + 鼠标滚轮调整界面缩放（0.5x ~ 3.0x）
- **中文界面** — 更纱黑体编译时嵌入，完整中文支持

## 构建

### 前置依赖

**Debian/Ubuntu：**

```bash
sudo apt-get install -y \
    libssl-dev pkg-config \
    libxkbcommon-dev libgl1-mesa-dev \
    libwayland-dev libxcb-shape0-dev libxcb-xfixes0-dev
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

## 使用

1. 启动应用，在左侧面板填写 SSH 连接信息（主机、端口、用户名、密码）
2. 点击「连接」建立 SSH 连接，自动加载远程目录下的 JSON 文件列表
3. 点击文件名加载任务图，右侧面板显示元数据和位姿编辑器
4. 编辑位姿数据，或点击位姿名称选中后通过按钮从 ROS2 获取实时数据
5. 右键文件可备份、删除，右键空白区域可上传本地文件
6. 点击「应用到远程文件」将修改写回远程文件

## 项目结构

```
src/
├── main.rs      # 入口：mimalloc 分配器、字体嵌入、窗口配置
├── model.rs     # 数据模型：位姿结构体、JSON 解析/序列化、登录持久化
├── app.rs       # GUI 应用：连接面板、文件列表（右键菜单）、位姿编辑器、响应轮询
├── worker.rs    # 后台工作线程：SSH/SFTP/ROS2 操作异步执行，mpsc 通信
└── ssh.rs       # SSH/SFTP：连接、文件操作、远程命令执行
```

## 发布

推送 `v*` 标签自动触发 CI 构建和发布：

```bash
git tag v0.3.0
git push origin v0.3.0
```

产物格式：

| 平台 | 文件名 |
|------|--------|
| Linux x64 | `task-graph-editor-v0.3.0+{commit}-linux-x64.tar.gz` |
| macOS ARM64 | `task-graph-editor-v0.3.0+{commit}-macos-arm64.tar.gz` |
| Windows x64 | `task-graph-editor-v0.3.0+{commit}-windows-x64.zip` |

## 许可证

MIT

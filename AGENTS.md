# 项目开发指南

本文件供开发 agent 共用，`CLAUDE.md` 是指向 `AGENTS.md` 的符号链接，修改此处即可同步。

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
│  ├─ ssh/                     # 原子写入、agent 认证与连接取消
│  ├─ ssh_config.rs            # ~/.ssh/config 解析：Host/Match/Include、首值生效、token 展开
│  ├─ worker.rs                # 后台工作线程：所有 SSH/SFTP/ROS2 操作在此异步执行（与 GUI 框架解耦）
│  └─ ui/
│     ├─ mod.rs                # 状态资源（Session/StatusLine/FileBrowser/Editor）、插件装配
│     ├─ theme.rs              # 四套主题、应用自定义 token、尺寸常量与切换
│     ├─ theme/
│     │  ├─ palette.rs         # 唯一内嵌色板的解析与校验
│     │  └─ refresh.rs         # 继承文字、光标及控件派生颜色刷新
│     ├─ theme_picker.rs       # 右上角主题菜单：固定四项，只更新资源与文字
│     ├─ theme_preferences.rs  # 独立主题偏好的加载与原子保存
│     ├─ fonts.rs              # 中文字体嵌入与全局覆盖
│     ├─ widgets.rs            # 通用构件：卡片、折叠区块、表单行、数值/文本/密码输入
│     ├─ password.rs           # 密码遮罩控件（Bevy 未提供，基于光标位置同步）
│     ├─ binding.rs            # 数值控件 ↔ TaskGraphData 的绑定与回写（纯逻辑，带单测）
│     ├─ shell.rs              # 界面骨架：顶栏 / 侧栏 / 内容区 / 状态栏 / 右键菜单层
│     ├─ connect.rs            # 连接面板：SSH 表单、ssh config 主机下拉、连接按钮组
│     ├─ worker_bridge.rs      # UI 动作、文档来源与后台响应
│     ├─ files.rs              # 文件列表：选中、右键菜单（上传/备份/删除）
│     ├─ editor.rs             # 字段树编辑器：六个分组、递归嵌套、懒加载
│     ├─ graph_layout.rs       # 流程图布局：拓扑分层、正交折线走线（纯逻辑，带单测）
│     ├─ graph_view.rs         # 流程图视图：节点卡片、边、面包屑下钻、详情栏
│     └─ screenshot.rs         # 截图：F12 手动 / TGE_SCREENSHOT 自动（CI 视觉回归）
├─ assets/fonts/               # 更纱黑体（SarasaTermSCNerd，编译时嵌入）
├─ assets/themes/palettes.json # 四套主题色板的唯一权威源，编译时嵌入
└─ scripts/
   ├─ build-release.sh         # 本地 Release 构建脚本
   ├─ deploy-remote.sh         # 远程部署脚本
   └─ test_deploy_remote.py    # 本地隔离的部署成功与失败注入测试
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
python3 scripts/test_deploy_remote.py  # 无真实 SSH 的部署回归

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

- `raw_json` 是合并基线。未编辑的字段保留原始 JSON 值，字符串化 JSON 逐字保留；修改时只合并
  已编辑的分量，位姿和轨迹中的未知扩展字段仍保留。不要把完整强类型结构覆盖回原始字段
- JSON `config.context` 中的位姿字段是**字符串化 JSON**，需要二次解析
- `config.context` 中所有字段自动识别类型（`ContextValue` 枚举，12 种变体）：
  - `Pose` — 字符串化 RobotPose（chassis/head/waist）
  - `Bool` / `Integer` / `Float` — 标量
  - `NumericArray` / `NumericArray2D` — 1D/2D 数值数组。真实任务图里两种写法都有
    （字符串化的 `"[0.01,0.17]"` 与原生 JSON 数组），`stringified` 记住原样，
    序列化按原形式写回，否则会悄悄改掉远程文件的数据格式
  - `JointTrajectory` — 原生 JSON 数组（positions + time_from_start）
  - `PoseArray` — 原生数组，元素支持位姿对象或字符串化位姿（如 `pick_poses`）。
    保存按原始元素各自的形式回写，未编辑的字符串逐字保留，编辑元素仍保留未知扩展字段
  - `NestedGroup` — 原生 JSON 对象，成员递归分类（如 `station_profiles.station_1.*`），支持任意层级嵌套
  - `Text` / `Null` / `RawJson` — 其他
- 未改的数组元素保留整数/浮点类型；字符串化数组修改后的整数仍写整数格式，原生数组的
  浮点元素不会收敛成整数。无法无损放入 f64 或 i64 的整数结构归入 `RawJson`，禁止截断精度
- **新增或编辑的字符串位姿以 position 在前**，四元数按 wxyz 排列；未知扩展字段保留。
  未编辑的字符串位姿保持原文，包括原有键顺序
- 模型序列化递归拒绝 NaN 和无穷大，不能依赖 serde_json 将它们静默写成 null
- `serde_json` 开了 `float_roundtrip`：默认的快速浮点解析有 1 ULP 误差
  （实测 `0.9216510910864573` 会被读成 `...572`），位姿坐标读一遍存回去就变了
- 解析覆盖率可用 `TGE_ANALYZE=<文件> cargo test 分析 -- --ignored --nocapture` 检查，
  会列出未能识别的字段并输出往返结果供比对
- 真实字符串位姿数组可用 `TASK_GRAPH_REAL_FILE=<本地副本> TGE_POSE_ARRAY_FIELD=pick_poses
  cargo test 真实文件位姿数组解析与绑定往返 -- --ignored --nocapture` 核验：逐项比对全部分量的
  f64 精度，并通过 UI 数值绑定逐个修改、往返解析，只操作内存副本，不回写原文件
- `config.nodes` / `config.edges` 另外解析成 `SubGraph`/`TaskNode` 供流程图展示，
  纯增量、不参与序列化（见 [流程图视图](#流程图视图)）
- 修改 `task_id` 后远程文件自动重命名为 `{task_id}.json`，拒绝路径分隔符与同名覆盖

### 文档来源与远程保存

- `Session::target` 和连接代次记录实际连接；表单改动不会改变当前连接的身份。
- 文件列表记录服务端 `realpath` 解析后的目录；加载时把连接代次、实际目录和文件名保存在
  `Editor::document`。保存只用这份来源，修改表单或浏览其他目录不会把旧文档写入新位置。
- 文件列表和元数据卡片分别显示浏览目录、文档保存位置。断开清除文档；新连接不能保存旧代次数据。
- 保存先在同目录独占创建临时文件，完整写入并确认权限、`fsync`、关闭，再原子替换旧文件。
  改名、上传和备份以无覆盖方式发布；新文件提交后才清理旧名，清理失败明确提示“已保存”及警告。
- **远端 Linux 需要 `python3` 和支持 `fsync@openssh.com` 的 SFTP 服务端**。提交脚本仅从 stdin JSON
  接收路径；不要把文件名拼进命令。前置能力不满足时停止保存，不能回退到 truncate 覆盖旧文件。
- 提交时若连接中断，回执可能丢失，应刷新核实目标状态；原子替换保证旧目标不会变成半写入文件。

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
- **一帧内按 采集输入 → 处理 → 重建 三段推进**（`UiSet::Input/Update/Rebuild`）。
  产生 `AppAction` 的 system 若排在处理它的 system 之后，消息要等下一帧才被读到；
  而窗口是 reactive 刷新的，没有新输入时下一帧可能是 5 秒后，表现就是"点了没反应"
- 输入与动作之间的 `worker_bridge::InputFlush` 提前执行 Bevy 文本编辑及密码同步。上游默认在
  PostUpdate 才应用文本，直接在 Update 保存会漏掉同帧输入；PostUpdate 仍负责重建产生的编辑。
  异步剪贴板仍未就绪时保留动作队列，临时按 16ms 唤醒；完成后自动执行并恢复原刷新策略。
  原始 `AppAction` 和可执行的 `DispatchAction` 分开，文件对话框也必须等待；断开立即取消排队动作。
- **点击命中的是最内层节点**：标记组件（`MenuAction`、`FileRow`、`PoseCard`）挂在外层容器上，
  直接拿 `On<Pointer<_>>` 的实体去查会查不到，observer 一 return 就成了点不动。
  统一用 `widgets::self_or_ancestor` 从命中处往上找
- **Feathers 按钮监听 `Activate`**：`FeathersButton` 使用 `ui_widgets::Button`，不带旧版
  `ui::Button` 的 `Interaction`；查询 `Changed<Interaction>` 会永远收不到它的点击。
  流程图切换和详情下钻统一处理 `Activate`，同时覆盖鼠标、Enter 和空格。
- **右键菜单不要走 picking 事件**：改读 `ButtonInput<MouseButton>` 判断右键、
  用 `Hovered`/`Interaction` 定位目标、`window.cursor_position()` 取位置。
  `Interaction` 只在挂了它的节点上更新，天然避开"命中子节点"的问题
- **属性变化不要当重建条件**。按钮的禁用态、选中高亮这类是属性，改组件即可
  （`widgets::ButtonGate` → `InteractionDisabled`）。把它们塞进重建条件会让状态一翻转就重建整块，
  正好撞上上面那个竞态——"点一下刷新列表冒出六个按钮"就是这么来的
- `ButtonGate` 同步也要覆盖跨帧新增按钮；不能只在状态资源变化时执行。
- **主机下拉先生成内容再展开**：Feathers 菜单依赖子项焦点保持打开；按钮一边打开菜单、一边
  刷新配置并销毁旧子项，会立即因丢焦关闭。`connect.rs` 保留 `MenuEvent` 的打开意图，等配置版本
  更新且 BSN 插槽就绪，再同时显示和设置焦点。空菜单也必须有可聚焦的提示项；等待期间主动唤醒窗口。
- 折叠标题的 observer 找到外层目标后停止传播，否则子文本与祖先会把同一次点击切换两次。
- 轨迹点、二维数组行的内容**懒加载**：展开时才建控件，避免大轨迹一次生成上千个输入框

### 数值绑定

数值控件挂 `ValueBinding { field_path, slot }`，描述"我编辑的是数据里的哪一个数"：
`field_path` 是索引路径（首元素索引顶层 `context_fields`，后续依次下钻嵌套分组），
`slot` 指明字段内部位置（位姿分量、数组下标、轨迹点关节等）。
控件发出 `ValueChange` 时由统一的 observer 按绑定回写。这部分是纯数据寻址，`binding.rs` 里有完整单测。

**数值一律走 f64**（`NumberFormat::F64`）、整数走 i64。ROS2 关节角是 9 位小数，f32 会丢精度。
数字框内部的 `EditableText` 要容纳完整 f64 十进制表示，等跨帧子控件就绪后再推初值。
`InputValidation` 记录当前文档中的非法数字文本，显示红色边框，并阻止保存或创建位姿引发整树重建；
`ValueChange` 和绑定层同时拒绝非有限值。只保留最后合法数值而不提示输入错误，会造成静默保存旧值。
ROS2 回填不得覆盖正在报错的输入，否则会悄悄清除用户尚未修正的文本及保存守卫。

### 界面结构

```
顶栏    ● 任务图编辑器       │ [参数 / 流程图] 当前文件       [编辑操作] [选择主题]
侧栏    SSH 连接卡片（表单 + ssh config 下拉 + 按钮组）
        文件列表卡片（单击加载，右键上传/备份/删除）
内容区  ┌ 参数视图：元数据卡片（map_id / task_id）
        │           Context 参数卡片 → 六个分组：位姿点位 / 基本参数 / 数组参数 / 轨迹数据 / 嵌套分组 / 其他
        └ 流程图视图：面包屑 + 本层统计 + 图例 / 画布 / 详情栏
状态栏  [复制提示] 状态消息 / 忙碌 / 重连（三选一）             user@host:port
```

窄窗口或放大 UI 后，编辑操作自动移到顶栏第二行；视图入口仍对齐右侧内容区左缘，主题菜单保持右上角。

顶栏与主体共用侧栏宽度：左侧是标题、SSH 和文件选择区，右侧是文档功能区。
「参数 / 流程图」入口常驻并对齐右侧内容的左缘，独立成组，与编辑操作按钮通过弹性间距分开。
编辑操作组可继续内部换行，顶栏随内容增高。两种视图互斥（`display`），未加载文档时显示引导。
选中高亮只更新 `ButtonVariant`，不重建按钮；跨帧生成的面板也要同步当前视图。

- 状态栏左侧永远只显示一条，优先级 重连 > 忙碌 > 最近一条结果——三者是同一件事的不同阶段，
  各占一边会变成「左下角和右下角都在说正在加载」；右侧放连接目标
- 左下角的「复制提示」复制当前显示消息的完整原文（含被裁剪的长文本和换行）；复制反馈只改按钮，
  保留原始提示。空消息禁用按钮，新消息清除旧复制反馈；新生成的 BSN 状态栏也必须同步初值。
- 位姿卡片点击选中（琥珀色高亮），选中后才能用顶栏的三个 ROS2 取数按钮
- 位姿的位置 xyz、姿态 wxyz 各横排一行，色条区分轴向（X 红 / Y 绿 / Z 蓝）
- UI 缩放：Shift + `+`/`-` 或 Shift + 滚轮（`UiScale`，0.5～3.0）
- 连接面板的主机框带下拉菜单，数据来自本机 `~/.ssh/config`（每次打开菜单重新解析，也可手动输入）；
  选中后填入 HostName/Port/User/IdentityFile，`ProxyJump`/`ProxyCommand` 条目会标注"跳板 · 不支持，将直连"
- 认证规则：密码非空 → 密码认证；密码为空 → 公钥认证（ssh-agent 全部身份 → 私钥框指定文件 →
  `~/.ssh/id_rsa`/`id_ecdsa`/`id_ed25519` 中存在者），全部失败时汇总每一步原因
- 按用户要求，连接和重连不校验服务器主机指纹，也不弹出信任确认。SSH 握手后直接进行
  密码或公钥认证；不读取或写入 `known_hosts`，已有用户信任文件保持原样。
- SSH config 中的 `Match final` / `canonical` 及其否定条件不执行：本工具没有 OpenSSH 的多阶段
  重解析过程，必须跳过这些块，不能当成恒真条件应用。
- DNS 与 TCP 建连共享 10 秒期限，SSH 会话 API 使用 30 秒期限；断开操作通过取消控制直接关闭
  已注册套接字，不能只在同一个阻塞 worker 的消息队列里排队等待断开。
- `ssh::agent` 的身份枚举、签名和网络认证共享 30 秒期限：Unix 使用非阻塞 socket，Windows 使用
  Pageant 有界消息及 OpenSSH 可取消管道，每 50ms 检查取消。Pageant 的消息线程最多存活至同一期限。
  libssh2 自定义签名回调集中在薄 FFI 层，持有会话锁时不能重入 Session；C 分配器分配的签名由 C 释放。

### 流程图视图

`config.nodes` / `config.edges` 描述了任务的执行流程，复合节点（`sequence`/`loop`/`parallel`/`condition` 等）
把子图**直接内联**在自己身上——是节点自带 `nodes`/`edges` 两个键，不是挂在 `children` 下面。

**只读**。本工具编辑的是 context 参数，流程由机器人端定义；`parse_task_graph` 只是多解析出一份
`TaskGraphData::graph`，序列化仍旧只从 `raw_json` 合并 context，`nodes`/`edges` 原样带回。

布局在 `graph_layout.rs`，纯函数、不碰 ECS；测试覆盖循环、跨层、自环和生成图的路径不变量：

- **分层**：先 DFS 找出闭环的回边并标记，再在无环子集上按**最长路径**定层
  （最短路径会让汇合点浮到上面，跨过它依赖的分支）
- **同层内**按文件里的书写顺序排，每层对齐到内容中线
- **走线**是正交折线。Bevy UI 没有画线的原语，一段折线拆成若干绝对定位的细矩形拼出来，
  终点补一个 `▼`/`▶`/`◀`/`▲` 字符当箭头
- **回边**走右侧通道，通道 x 贴着**这条边纵向跨过的那几层**里最宽的一层，
  不是全图最宽层——否则一条只穿过单列区域的回边会被甩到几百像素外，看着像断掉的线
- **跨层边与自环**也使用外侧通道，边的所有线段避开非端点卡片，通道占用计入画布边界。

几个不显然的点：

- **文件顺序 ≠ 拓扑顺序**。顶层 `log_init_complete` 写在第 4 位，但边把它排在第 7 层。
  看着"顺序不对"时先核对 `edges`，别急着改布局
- **画布要主动居中**。每层都对齐到内容中线，而 `ScrollPosition` 默认停在最左：
  内容比视口宽时主干会被推到右边缘、右侧通道整条看不见。`center_canvas` 靠标记组件
  轮询到布局尺寸后居中一次再摘掉标记（摘掉是为了不跟用户后续的手动滚动打架）。
  **`ComputedNode` 是物理像素，`ScrollPosition` 是逻辑像素**，要乘 `inverse_scale_factor`，
  否则在 2x 屏上滚过头一倍
- **详情栏按需出现**。窗口 1280 宽，左栏 ~320 + 详情栏 300，画布只剩 660——3 宽的层就装不下。
  没选中节点时不占那 300px，操作提示挪到工具栏（否则没人知道节点可以点）
- **节点 id 最长 73 字符**，中位 27，没有任何卡片宽度能全放下。凡是显示 id 的地方
  都要 `LineBreak::AnyCharacter`：标识符不含空格，按词换行等于不换行，会直接顶破容器。
  详情栏因此用标签在上、值在下的竖排，不复用 `widgets::field_row`（那是 190px 固定标签列的横排）
- **换文件回根层**：`reset_on_reload` 比对 `editor.structure_version`，变了就 `nav.reset()`
- **选中不重建画布**：只同步节点边框和详情栏，保留画布实体及 `ScrollPosition`；导航下钻才更新
  布局版本并重建，不能用统一导航版本表达单纯的选中变化。

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
| `libc` | Unix 非阻塞 agent 套接字及 libssh2 签名回调的配套 C 内存分配 |
| `windows-sys`（仅 Windows） | Pageant 消息、共享映射及 OpenSSH 管道的取消控制 |
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
- **主题切换**：默认石墨青，右上角「选择主题」提供石墨青、暮砂金、暖纸橙、雾白靛。
  `ThemeSelection` 改变后更新 `UiTheme`，同帧刷新颜色，不重建控件，不因换色重置焦点、光标或滚动位置。
  `theme/refresh.rs` 补齐上游未随资源换色刷新的继承文字、文本光标、滑块和图标，并修复输入框默认白字。
  自定义菜单 caption 的中间容器也要加 `ThemedText`，否则颜色传播会在容器处中断，浅色菜单留下白字。
  四个菜单项与入口一起内联生成，选择后仅同步资源、名称和勾选；不要照搬主机菜单的动态子项重建。
- **色板维护**：只改 `assets/themes/palettes.json`；`theme/palette.rs` 通过 `include_str!` 嵌入并校验，
  `docs/theme-proposals/palettes.json` 是指向它的相对符号链接。颜色使用不透明的 sRGB `#RRGGBB`，
  配对关系、柔化边框及对比度检查见 [主题文档](docs/theme-proposals/README.md)。
- **主题记忆**：`ThemePreferencesPlugin` 在 `PreStartup` 读取独立的
  `~/.config/task-graph-editor/theme.json`；只有实际选择变化才在 `UiSet::Rebuild` 后原子保存。
  启动默认值和恢复值不写入；保留未知键，未知主题回退默认，损坏 JSON 禁止覆盖。
  保存失败不逐帧重试；主目录不可得时禁用存储，不能回退工作目录。
  离线测试和截图夹具必须在插件装配前注入 `ThemePreferencesStore::disabled()`；
  持久化测试使用 `at_path` 指定隔离临时路径，不访问使用者配置。
- **Windows**：Release 构建隐藏控制台窗口（`windows_subsystem = "windows"`）
- **profile**：依赖用 `opt-level = 3` 编译（dev 与 release 都是），Bevy 在低优化下交互明显发涩

## CI 流水线

四阶段串行（GitHub Actions + GitLab CI）：

1. **Format**：`cargo fmt --all -- --check`
2. **Clippy**：`cargo clippy --all --all-targets -- -D warnings`
3. **Test**：`cargo test --all` + 部署脚本隔离回归
4. **Build**（三平台并行）：Linux x64 / macOS ARM64 / Windows x64

- 工具链：Rust nightly
- CI 的 Clippy、测试与构建使用 `--locked`，严格复用已提交的依赖组合。
  升级 Bevy/wgpu 后需检查 Windows 依赖树：`wgpu-hal` 和 `gpu-allocator` 之间共享 DX12 类型，
  必须解析到同一版 `windows`。当前统一为 0.62.2；保留旧的 0.61.x 会导致跨 crate 类型不匹配
- 用 `CARGO_BUILD_WARNINGS=deny` 拦截构建警告，保持编译指纹不变；CI 关闭增量编译
- GitLab 的 MR、分支和标签质量任务条件一致，构建 `needs` 不能指向缺失的测试任务
- GitHub 的开发构建、PR 和发布构建都上传产物，开发摘要指向本次运行的下载入口
- 缓存：Swatinem/rust-cache@v2
- Linux 系统依赖：`libxkbcommon-dev libgl1-mesa-dev libwayland-dev libx11-dev libxcursor-dev
  libxrandr-dev libxi-dev`（不需要 libasound2-dev / libudev-dev —— 音频与手柄 feature 都没开）
- UPX 压缩：仅 Linux `--best --lzma`；macOS 不支持；Windows 跳过（UPX 加壳的无签名 exe 会触发 Defender/SmartScreen 木马误报）
- 产物命名：`task-graph-editor-{版本}-{平台}.{扩展名}`
- 推送 `v*` 标签自动创建 Release（含 prerelease 检测）
- Unix 本地原子提交测试和部署回归需要 `python3`；Windows 跳过 Unix 系统调用集成测试
- 部署脚本远端启用严格错误退出，失败保留现有可执行文件和上传包；先在临时位置准备、校验，再替换

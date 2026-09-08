//! Bevy UI 层：状态资源、插件装配与各面板模块
//!
//! 与旧版 egui 即时模式的区别：UI 是保留模式的实体树。
//! 状态集中放在几个 Resource 里，控件树按"结构版本号"增量重建，
//! 纯数值变化只推送到对应控件，不重建树（否则输入焦点会丢失）。

use bevy::prelude::*;

use crate::model::{LoginConfig, TaskGraphData};
use crate::ssh_config::{self, SshHostEntry};
use crate::worker::{BusyState, WorkerHandle};
use binding::PoseTarget;

pub mod binding;
pub mod connect;
pub mod editor;
pub mod files;
pub mod fonts;
pub mod graph_layout;
pub mod graph_view;
pub mod password;
pub mod screenshot;
pub mod shell;
pub mod theme;
pub mod theme_picker;
pub mod theme_preferences;
pub mod widgets;
pub mod worker_bridge;

/// 等待中的远程命令类型（用于识别 `CommandOutput` 响应的来源）
///
/// 保存发起时的完整位姿目标及结构版本，返回时不读取当前选中，
/// 也不能将旧请求写入重载后恰好位于相同索引的新文档。
pub enum PendingCommand {
    /// 获取底盘位姿
    ChassisPose {
        target: PoseTarget,
        structure_version: u64,
    },
    /// 获取头部关节角
    HeadJoints {
        target: PoseTarget,
        structure_version: u64,
    },
    /// 获取腰部关节角
    WaistJoints {
        target: PoseTarget,
        structure_version: u64,
    },
}

/// 已发起连接的目标快照，不随连接表单编辑而变化。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionTarget {
    pub host: String,
    pub port: u16,
    pub username: String,
}

impl std::fmt::Display for ConnectionTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}:{}", self.username, self.host, self.port)
    }
}

/// 已加载文档的来源；文件操作不能重新读取可编辑的连接表单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDocument {
    pub connection_generation: u64,
    pub remote_dir: String,
    pub filename: String,
}

/// SSH 会话与连接表单状态
#[derive(Resource)]
pub struct Session {
    /// 连接表单内容（同时用于本地持久化）
    pub login: LoginConfig,
    /// 后台工作线程句柄
    pub worker: Option<WorkerHandle>,
    /// 每次手动建立或断开连接时递增，拒绝将旧文档写到新会话。
    pub connection_generation: u64,
    /// 实际连接目标，供状态栏显示。
    pub target: Option<ConnectionTarget>,
    /// 当前忙碌状态
    pub busy: BusyState,
    /// 是否已连接（由 worker 响应驱动）
    pub is_connected: bool,
    /// 自动重连状态描述（非 None 表示正在自动重连中）
    pub reconnect_status: Option<String>,
    /// 等待中的远程命令
    pub pending_command: Option<PendingCommand>,
    /// `~/.ssh/config` 中解析出的主机列表（启动时读取，每次打开下拉菜单时刷新）
    pub ssh_hosts: Vec<SshHostEntry>,
    /// 主机下拉菜单版本号：刷新主机列表后递增，驱动菜单项重建
    pub hosts_version: u64,
    /// 表单值版本号：程序改动表单内容后递增（如套用 ssh config 主机），
    /// 驱动把新值刷回输入框。用户自己打字不递增，否则会打断输入。
    pub form_version: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(
            crate::model::load_login_config(),
            ssh_config::load_user_hosts(),
        )
    }
}

impl Session {
    /// 用显式配置创建会话，测试无需读取使用者的登录文件和 SSH 配置。
    pub fn new(login: LoginConfig, ssh_hosts: Vec<SshHostEntry>) -> Self {
        Self {
            login,
            worker: None,
            connection_generation: 0,
            target: None,
            busy: BusyState::Idle,
            is_connected: false,
            reconnect_status: None,
            pending_command: None,
            ssh_hosts,
            hosts_version: 0,
            form_version: 0,
        }
    }
    /// 判断当前是否忙碌（有请求正在后台执行）
    pub fn is_busy(&self) -> bool {
        !matches!(self.busy, BusyState::Idle)
    }

    /// 忙碌状态描述文字（带上正在处理的对象，便于一眼看出卡在哪一步）
    pub fn busy_text(&self) -> String {
        match &self.busy {
            BusyState::Idle => String::new(),
            BusyState::Connecting => "正在连接...".into(),
            BusyState::Refreshing => "正在刷新文件列表...".into(),
            BusyState::Loading(name) => format!("正在加载 {name}..."),
            BusyState::Working(what) => format!("{what}..."),
            BusyState::Fetching(what) => format!("正在获取{what}..."),
        }
    }

    /// 交互是否可用：未忙碌且不在自动重连中
    pub fn interactive(&self) -> bool {
        !self.is_busy() && self.reconnect_status.is_none()
    }

    /// 外部读取到的域值统一刷新表单，避免实际命令与屏幕内容不一致。
    pub fn update_ros_domain_id(&mut self, value: Option<String>) {
        if let Some(value) = value
            && self.login.ros_domain_id != value
        {
            self.login.ros_domain_id = value;
            self.form_version += 1;
        }
    }

    /// 发送请求到后台线程
    pub fn send(&self, request: crate::worker::WorkerRequest) {
        match &self.worker {
            Some(worker) => worker.send(request),
            None => tracing::warn!("未连接，请求被丢弃"),
        }
    }

    /// 保存当前登录信息到本地配置文件
    pub fn save_login(&self) {
        crate::model::save_login_config(&self.login);
    }
}

/// 状态栏消息
#[derive(Resource, Default)]
pub struct StatusLine {
    /// 显示文本
    pub text: String,
}

/// 状态消息的语义级别，决定状态栏配色
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    /// 正常/成功
    Ok,
    /// 注意/警告
    Warn,
    /// 失败/错误
    Error,
}

impl StatusLine {
    /// 设置状态文本
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }

    /// 由文本关键词推断级别（沿用旧版的判定规则）
    pub fn level(&self) -> StatusLevel {
        let has_any = |keywords: &[&str]| keywords.iter().any(|k| self.text.contains(k));
        match (has_any(&["失败", "错误"]), has_any(&["注意", "警告"])) {
            (true, _) => StatusLevel::Error,
            (false, true) => StatusLevel::Warn,
            (false, false) => StatusLevel::Ok,
        }
    }
}

/// 远程文件浏览状态
#[derive(Resource, Default)]
pub struct FileBrowser {
    /// 当前列表对应的规范化绝对目录；未成功列目录时不可操作旧条目。
    pub remote_dir: Option<String>,
    /// 远程目录下的 JSON 文件名
    pub files: Vec<String>,
    /// 当前选中的文件
    pub selected: Option<String>,
    /// 列表内容版本号：文件列表变化时递增，驱动列表重建
    pub list_version: u64,
}

impl FileBrowser {
    /// 替换文件列表并递增版本号
    pub fn set_files(&mut self, files: Vec<String>) {
        self.files = files;
        self.list_version += 1;
    }
}

/// 任务图编辑状态
#[derive(Resource, Default)]
pub struct Editor {
    /// 当前编辑的数据
    pub data: Option<TaskGraphData>,
    /// 当前文档绑定的远程来源，与侧栏正在浏览的目录相互独立。
    pub document: Option<RemoteDocument>,
    /// 当前选中的独立位姿或位姿数组元素，支持嵌套分组下钻。
    pub selected_pose: Option<PoseTarget>,
    /// 结构版本号：字段树形状变化时递增（加载文件、创建位姿），驱动编辑器重建
    ///
    /// 单纯的数值编辑不递增，否则输入焦点会在每次按键后丢失。
    pub structure_version: u64,
    /// 数值版本号：数据被 UI 之外的来源改动时递增（如 ROS2 回填），
    /// 驱动已有控件刷新显示，但不重建控件树。
    pub value_version: u64,
}

impl Editor {
    /// 载入新数据（重置选中并递增结构版本）
    pub fn load(&mut self, data: Option<TaskGraphData>) {
        self.data = data;
        self.document = None;
        self.selected_pose = None;
        self.structure_version += 1;
    }

    /// 加载远程文档并记住其来源。
    pub fn load_remote(&mut self, data: TaskGraphData, document: RemoteDocument) {
        self.load(Some(data));
        self.document = Some(document);
    }

    /// 标记字段树结构已变化，需要重建控件
    pub fn mark_structure_changed(&mut self) {
        self.structure_version += 1;
    }

    /// 标记数值被 UI 之外的来源改过，需要刷新控件显示
    pub fn mark_values_changed(&mut self) {
        self.value_version += 1;
    }

    /// 检查完整选中目标仍指向一个位姿。
    pub fn has_pose_selection(&self) -> bool {
        self.selected_pose.as_ref().is_some_and(|target| {
            self.data
                .as_ref()
                .is_some_and(|data| target.pose(data).is_some())
        })
    }
}

/// 系统集：一帧内按 采集输入 → 处理 → 重建 的顺序推进
///
/// 三段必须分开：产生 `AppAction` 的 system 若排在处理它的 system 之后，
/// 消息就要等下一帧才被读到——而窗口是 reactive 刷新的，没有新输入时
/// 下一帧可能是 5 秒之后，表现就是"点了半天没反应"。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum UiSet {
    /// 采集鼠标/键盘输入，产生 `AppAction`
    Input,
    /// 处理 `AppAction` 与后台线程响应，更新状态资源
    Update,
    /// 按状态重建/刷新控件树
    Rebuild,
}

/// 任务图编辑器 UI 插件
pub struct EditorUiPlugin;

impl Plugin for EditorUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Session>()
            .init_resource::<StatusLine>()
            .init_resource::<FileBrowser>()
            .init_resource::<Editor>()
            .configure_sets(
                Update,
                (UiSet::Input, UiSet::Update, UiSet::Rebuild).chain(),
            )
            .add_plugins((
                theme::ThemePlugin,
                theme_preferences::ThemePreferencesPlugin,
                theme_picker::ThemePickerPlugin,
                widgets::WidgetsPlugin,
                screenshot::ScreenshotPlugin,
                fonts::FontsPlugin,
                password::PasswordInputPlugin,
                worker_bridge::WorkerBridgePlugin,
                shell::ShellPlugin,
                connect::ConnectPanelPlugin,
                files::FileListPlugin,
                editor::EditorPanelPlugin,
                graph_view::GraphViewPlugin,
            ));
    }
}

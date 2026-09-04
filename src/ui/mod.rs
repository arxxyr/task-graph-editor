//! Bevy UI 层：状态资源、插件装配与各面板模块
//!
//! 与旧版 egui 即时模式的区别：UI 是保留模式的实体树。
//! 状态集中放在几个 Resource 里，控件树按"结构版本号"增量重建，
//! 纯数值变化只推送到对应控件，不重建树（否则输入焦点会丢失）。

use bevy::prelude::*;

use crate::model::{LoginConfig, TaskGraphData};
use crate::ssh_config::{self, SshHostEntry};
use crate::worker::{BusyState, WorkerHandle};

pub mod binding;
pub mod connect;
pub mod editor;
pub mod files;
pub mod fonts;
pub mod password;
pub mod screenshot;
pub mod shell;
pub mod theme;
pub mod widgets;
pub mod worker_bridge;

/// 等待中的远程命令类型（用于识别 `CommandOutput` 响应的来源）
///
/// `field_path` 是索引路径：首元素索引顶层 `context_fields`，
/// 后续元素依次下钻嵌套分组（见 [`crate::model::field_at_path`]）。
pub enum PendingCommand {
    /// 获取底盘位姿
    ChassisPose { field_path: Vec<usize> },
    /// 获取头部关节角
    HeadJoints { field_path: Vec<usize> },
    /// 获取腰部关节角
    WaistJoints { field_path: Vec<usize> },
}

/// SSH 会话与连接表单状态
#[derive(Resource)]
pub struct Session {
    /// 连接表单内容（同时用于本地持久化）
    pub login: LoginConfig,
    /// 后台工作线程句柄
    pub worker: Option<WorkerHandle>,
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
        Self {
            login: crate::model::load_login_config(),
            worker: None,
            busy: BusyState::Idle,
            is_connected: false,
            reconnect_status: None,
            pending_command: None,
            ssh_hosts: ssh_config::load_user_hosts(),
            hosts_version: 0,
            form_version: 0,
        }
    }
}

impl Session {
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
    /// 当前选中的位姿字段索引路径（支持嵌套分组下钻，仅用于 Pose 类型）
    pub selected_pose_path: Option<Vec<usize>>,
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
        self.selected_pose_path = None;
        self.structure_version += 1;
    }

    /// 标记字段树结构已变化，需要重建控件
    pub fn mark_structure_changed(&mut self) {
        self.structure_version += 1;
    }

    /// 标记数值被 UI 之外的来源改过，需要刷新控件显示
    pub fn mark_values_changed(&mut self) {
        self.value_version += 1;
    }

    /// 检查选中字段是否为 Pose 类型（支持嵌套分组路径）
    pub fn has_pose_selection(&self) -> bool {
        self.selected_pose_path.as_ref().is_some_and(|path| {
            self.data.as_ref().is_some_and(|data| {
                matches!(
                    crate::model::field_at_path(&data.context_fields, path).map(|f| &f.value),
                    Some(crate::model::ContextValue::Pose(_))
                )
            })
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
                widgets::WidgetsPlugin,
                screenshot::ScreenshotPlugin,
                fonts::FontsPlugin,
                password::PasswordInputPlugin,
                worker_bridge::WorkerBridgePlugin,
                shell::ShellPlugin,
                connect::ConnectPanelPlugin,
                files::FileListPlugin,
                editor::EditorPanelPlugin,
            ));
    }
}

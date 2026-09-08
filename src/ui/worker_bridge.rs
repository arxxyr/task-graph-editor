//! UI 与后台 SSH 线程之间的桥接
//!
//! 三件事：
//! 1. 把界面上的操作意图收敛成 [`AppAction`] 消息，统一在一处翻译成 `WorkerRequest`；
//! 2. 每帧轮询后台线程的响应，更新状态资源；
//! 3. 提供跨线程唤醒回调——后台线程有响应时通过 winit 的 `EventLoopProxy`
//!    把主循环从"无输入不重绘"的休眠中叫醒。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::text::EditableText;
use bevy::winit::{EventLoopProxyWrapper, UpdateMode, WinitSettings, WinitUserEvent};

use crate::model::{self, ContextValue, RobotPose};
use crate::ssh::{AuthMethod, DirListing};
use crate::ssh_config;
use crate::worker::{BusyState, WakeFn, WorkerHandle, WorkerRequest, WorkerResponse};

use super::binding::PoseTarget;
use super::editor::InputValidation;
use super::{
    ConnectionTarget, Editor, FileBrowser, PendingCommand, RemoteDocument, Session, StatusLine,
    UiSet,
};

/// ROS2 环境 source 前缀（不含 ROS_DOMAIN_ID，运行时动态拼接）
const ROS_ENV_PREFIX: &str =
    "source /opt/ros/humble/setup.bash && source /opt/spiderrobot/setup.bash";

/// 获取头部/腰部关节角的 Python 脚本（通过 stdin 传给远程 python3）
const JOINT_STATES_SCRIPT: &str = r#"
import rclpy
from rclpy.node import Node
from rclpy.executors import SingleThreadedExecutor
from sensor_msgs.msg import JointState

TARGETS = ("head_joint_1", "head_joint_2", "body_joint_1", "body_joint_2")

class JointPick(Node):
    def __init__(self):
        super().__init__("joint_pick")
        self.latest = {k: None for k in TARGETS}
        self.done = False
        self.create_subscription(JointState, "/joint_states", self.cb, 10)

    def cb(self, msg):
        for idx, n in enumerate(msg.name):
            if n in self.latest and idx < len(msg.position):
                self.latest[n] = float(msg.position[idx])
        if (not self.done) and all(self.latest[k] is not None for k in TARGETS):
            self.done = True

def main():
    rclpy.init()
    node = JointPick()
    ex = SingleThreadedExecutor()
    ex.add_node(node)
    try:
        while rclpy.ok() and not node.done:
            ex.spin_once(timeout_sec=0.2)
        if node.done:
            print(
                f"head_joint_1={node.latest['head_joint_1']:.9f} "
                f"head_joint_2={node.latest['head_joint_2']:.9f} "
                f"body_joint_1={node.latest['body_joint_1']:.9f} "
                f"body_joint_2={node.latest['body_joint_2']:.9f}",
                flush=True)
    finally:
        ex.remove_node(node)
        node.destroy_node()
        rclpy.shutdown()

if __name__ == "__main__":
    main()
"#;

/// 界面操作意图
///
/// 所有按钮、菜单项都只发这个消息，具体怎么做在 [`handle_actions`] 里统一处理，
/// 避免副作用散落在各个 observer 里。
#[derive(Message, Debug, Clone)]
pub enum AppAction {
    /// 连接远程主机
    Connect,
    /// 断开连接（同时取消自动重连）
    Disconnect,
    /// 刷新远程文件列表
    RefreshFiles,
    /// 选择本地文件上传
    UploadFile,
    /// 加载指定远程文件
    LoadFile(String),
    /// 备份指定远程文件
    BackupFile(String),
    /// 删除指定远程文件
    DeleteFile(String),
    /// 将编辑结果写回远程
    SaveToRemote,
    /// 从 ROS2 获取底盘位姿
    FetchChassisPose,
    /// 从 ROS2 获取头部关节角
    FetchHeadJoints,
    /// 从 ROS2 获取腰部关节角
    FetchWaistJoints,
    /// 选中独立位姿或位姿数组中的一个元素。
    SelectPose(PoseTarget),
    /// 在 null 字段上创建一个默认位姿
    CreatePose(Vec<usize>),
    /// 重新读取 `~/.ssh/config`
    ReloadSshHosts,
    /// 套用 ssh config 中的第 n 个主机条目
    ApplySshHost(usize),
}

/// 文本输入完成后才能执行的动作，和原始输入消息分开以免文件对话框提前提交。
#[derive(Message)]
struct DispatchAction(AppAction);

/// 异步粘贴期间保留用户的一次点击，并暂存应用原来的空闲刷新策略。
#[derive(Resource, Default)]
struct PendingActions {
    actions: VecDeque<AppAction>,
    update_settings: Option<WinitSettings>,
}

const CLIPBOARD_WAIT_MESSAGE: &str = "正在读取剪贴板，完成后自动继续操作...";
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(16);

/// 输入刷新只能推进已就绪的粘贴，尚在读取时必须保留依赖这些输入的操作。
fn dispatch_after_text_input(
    mut incoming: MessageReader<AppAction>,
    texts: Query<&EditableText>,
    mut pending: ResMut<PendingActions>,
    mut status: ResMut<StatusLine>,
    mut dispatched: MessageWriter<DispatchAction>,
    settings: Option<ResMut<WinitSettings>>,
) {
    for action in incoming.read() {
        match action {
            AppAction::Disconnect => {
                // 用户主动取消时，不能在稍后粘贴完成后又执行旧的保存或连接意图。
                pending.actions.clear();
                dispatched.write(DispatchAction(action.clone()));
            }
            _ => pending.actions.push_back(action.clone()),
        }
    }

    let waiting = texts.iter().any(|text| text.pending_paste.is_some());
    if waiting {
        if !pending.actions.is_empty() {
            status.set(CLIPBOARD_WAIT_MESSAGE);
        }
        if let Some(mut settings) = settings
            && pending.update_settings.is_none()
        {
            pending.update_settings = Some(settings.clone());
            // winit 在本帧结束后重读设置并安排下一次唤醒，失焦也不等数十秒。
            settings.focused_mode = UpdateMode::reactive(CLIPBOARD_POLL_INTERVAL);
            settings.unfocused_mode = UpdateMode::reactive_low_power(CLIPBOARD_POLL_INTERVAL);
        }
        return;
    }

    if let Some(original) = pending.update_settings.take()
        && let Some(mut settings) = settings
    {
        *settings = original;
    }
    if status.text == CLIPBOARD_WAIT_MESSAGE {
        status.set("剪贴板读取完成");
    }
    for action in pending.actions.drain(..) {
        dispatched.write(DispatchAction(action));
    }
}

/// 构建远程 ROS2 命令（自动加 source 环境 + ROS_DOMAIN_ID）
fn ros_cmd(session: &Session, cmd: &str) -> String {
    format!(
        "bash -c 'export ROS_DOMAIN_ID={} && {ROS_ENV_PREFIX} && {cmd}'",
        session.login.ros_domain_id
    )
}

/// 根据表单决定认证方式：密码非空 → 密码认证；否则公钥认证（ssh-agent → 私钥文件）
fn auth_method(session: &Session) -> AuthMethod {
    if !session.login.password.is_empty() {
        return AuthMethod::Password(session.login.password.clone());
    }
    let identity_file = session.login.identity_file.trim();
    let identity_files = match identity_file.is_empty() {
        true => ssh_config::default_identity_files(),
        false => vec![ssh_config::expand_user_path(identity_file)],
    };
    AuthMethod::PublicKey { identity_files }
}

/// 校验当前选中位姿并返回完整目标（无有效选中时写状态提示）。
fn validated_pose_target(editor: &Editor, status: &mut StatusLine) -> Option<PoseTarget> {
    let Some(target) = editor.selected_pose.clone() else {
        status.set("请先选中一个位姿点位");
        return None;
    };
    if !editor.has_pose_selection() {
        status.set("选中的字段不是位姿类型");
        return None;
    }
    Some(target)
}

/// 把 ssh config 中的主机条目填入连接表单（密码保持不动，由用户自行决定）
pub(super) fn apply_ssh_host(session: &mut Session, status: &mut StatusLine, index: usize) {
    let Some(entry) = session.ssh_hosts.get(index).cloned() else {
        return;
    };
    session.login.host = entry.host_name.clone();
    session.login.port = entry.port.to_string();
    // 无 User 时按 OpenSSH 语义回落到本地用户名
    if let Some(user) = entry.user.clone().or_else(ssh_config::local_user_name) {
        session.login.username = user;
    }
    // 优先取磁盘上真实存在的私钥；都不存在则填第一个，让后续连接错误一目了然
    session.login.identity_file = entry
        .identity_files
        .iter()
        .find(|path| path.is_file())
        .or_else(|| entry.identity_files.first())
        .map(|path| path.display().to_string())
        .unwrap_or_default();

    let proxy_note = match (&entry.proxy_jump, &entry.proxy_command) {
        (Some(jump), _) => {
            format!("；注意：该主机配置了 ProxyJump {jump}，本工具不支持跳板，将直连")
        }
        (None, Some(command)) => {
            format!("；注意：该主机配置了 ProxyCommand（{command}），本工具不支持代理命令，将直连")
        }
        (None, None) => String::new(),
    };
    // 表单是被程序改的，要把新值刷回输入框
    session.form_version += 1;
    status.set(format!(
        "已载入 ssh config 主机 {}：{}@{}:{}{proxy_note}",
        entry.alias, session.login.username, session.login.host, session.login.port
    ));
}

/// 将远程数据写入发起时的位姿目标，成功时返回含数组下标的路径。
fn apply_to_pose_at(
    editor: &mut Editor,
    target: &PoseTarget,
    apply: impl FnOnce(&mut RobotPose),
) -> Option<String> {
    let data = editor.data.as_mut()?;
    let pose = target.pose_mut(data)?;
    apply(pose);
    Some(target.display_path(data))
}

/// 创建跨线程唤醒回调
///
/// 窗口在无输入时不重绘，后台线程有响应就靠这个把事件循环叫醒。
fn make_wake_fn(proxy: &EventLoopProxyWrapper) -> WakeFn {
    let proxy = (**proxy).clone();
    Arc::new(move || {
        let _ = proxy.send_event(WinitUserEvent::WakeUp);
    })
}

/// 远程动作必须在业务入口再次校验，不能只依赖跨帧生成的按钮禁用态。
fn needs_connection(action: &AppAction) -> bool {
    matches!(
        action,
        AppAction::RefreshFiles
            | AppAction::UploadFile
            | AppAction::LoadFile(_)
            | AppAction::BackupFile(_)
            | AppAction::DeleteFile(_)
            | AppAction::SaveToRemote
            | AppAction::FetchChassisPose
            | AppAction::FetchHeadJoints
            | AppAction::FetchWaistJoints
    )
}

/// 将文档来源快照转换为保存请求；浏览目录和连接表单不参与寻址。
fn save_request(session: &Session, editor: &Editor) -> Result<WorkerRequest, String> {
    let document = editor
        .document
        .as_ref()
        .ok_or("当前文档没有远程来源，请重新加载")?;
    if !session.is_connected || document.connection_generation != session.connection_generation {
        return Err("连接已变化，请从当前主机重新加载文件后再保存".into());
    }
    let data = editor.data.as_ref().ok_or("无数据可保存")?;
    if data.task_id.trim().is_empty() {
        return Err("task_id 不能为空".into());
    }
    let new_name = format!("{}.json", data.task_id);
    crate::worker::validate_filename(&new_name)?;
    let content = model::serialize_task_graph(data).map_err(|e| format!("序列化失败: {e}"))?;
    Ok(WorkerRequest::SaveFile {
        remote_dir: document.remote_dir.clone(),
        current_filename: document.filename.clone(),
        content,
        new_filename: (new_name != document.filename).then_some(new_name),
    })
}

/// 断开时清除连接与文档状态，旧 worker 的响应不会进入新会话。
fn disconnect(session: &mut Session, browser: &mut FileBrowser, editor: &mut Editor) {
    session.send(WorkerRequest::Disconnect);
    session.worker = None;
    session.connection_generation += 1;
    session.target = None;
    session.is_connected = false;
    session.reconnect_status = None;
    session.pending_command = None;
    session.busy = BusyState::Idle;
    browser.set_files(Vec::new());
    browser.remote_dir = None;
    browser.selected = None;
    editor.load(None);
}

/// 处理界面操作意图
fn handle_actions(
    mut actions: MessageReader<DispatchAction>,
    mut session: ResMut<Session>,
    mut status: ResMut<StatusLine>,
    mut browser: ResMut<FileBrowser>,
    mut editor: ResMut<Editor>,
    validation: Res<InputValidation>,
    proxy: Option<Res<EventLoopProxyWrapper>>,
) {
    for DispatchAction(action) in actions.read() {
        debug!(?action, "处理界面操作");
        if needs_connection(action) && (!session.is_connected || !session.interactive()) {
            status.set("当前连接不可操作，请等待操作完成或重新连接");
            continue;
        }
        match action {
            AppAction::Connect => {
                if !session.interactive() || session.is_connected {
                    continue;
                }
                let Some(proxy) = &proxy else {
                    status.set("连接失败：窗口事件循环尚未就绪");
                    continue;
                };
                status.set("");
                let port = match session.login.port.parse::<u16>() {
                    Ok(port) if port != 0 => port,
                    _ => {
                        status.set("连接失败：端口必须是 1～65535 的整数");
                        continue;
                    }
                };
                // 每次连接创建新的 worker 线程
                let worker = match WorkerHandle::spawn(make_wake_fn(proxy)) {
                    Ok(worker) => worker,
                    Err(error) => {
                        status.set(format!("启动后台线程失败: {error}"));
                        continue;
                    }
                };
                session.connection_generation += 1;
                session.target = Some(ConnectionTarget {
                    host: session.login.host.clone(),
                    port,
                    username: session.login.username.clone(),
                });
                browser.remote_dir = None;
                browser.selected = None;
                browser.set_files(Vec::new());
                editor.load(None);
                worker.send(WorkerRequest::Connect {
                    host: session.login.host.clone(),
                    port,
                    username: session.login.username.clone(),
                    auth: auth_method(&session),
                    remote_dir: session.login.remote_dir.clone(),
                });
                session.worker = Some(worker);
                session.busy = BusyState::Connecting;
            }

            AppAction::Disconnect => {
                disconnect(&mut session, &mut browser, &mut editor);
                status.set("已断开连接");
            }

            AppAction::RefreshFiles => {
                session.busy = BusyState::Refreshing;
                session.send(WorkerRequest::RefreshFiles {
                    remote_dir: session.login.remote_dir.clone(),
                });
            }

            AppAction::LoadFile(filename) => {
                let Some(remote_dir) = browser.remote_dir.clone() else {
                    status.set("请先刷新文件列表");
                    continue;
                };
                session.busy = BusyState::Loading(filename.clone());
                status.set("");
                browser.selected = Some(filename.clone());
                session.send(WorkerRequest::LoadFile {
                    remote_dir,
                    filename: filename.clone(),
                });
            }

            AppAction::BackupFile(filename) => {
                let Some(remote_dir) = browser.remote_dir.clone() else {
                    status.set("请先刷新文件列表");
                    continue;
                };
                session.busy = BusyState::Working(format!("正在备份 {filename}"));
                status.set("");
                session.send(WorkerRequest::BackupFile {
                    remote_dir,
                    filename: filename.clone(),
                    existing_files: browser.files.clone(),
                });
            }

            AppAction::DeleteFile(filename) => {
                let Some(remote_dir) = browser.remote_dir.clone() else {
                    status.set("请先刷新文件列表");
                    continue;
                };
                session.busy = BusyState::Working(format!("正在删除 {filename}"));
                status.set("");
                session.send(WorkerRequest::DeleteFile {
                    remote_dir,
                    filename: filename.clone(),
                });
            }

            // 要弹原生文件对话框，只能在主线程做，交给 handle_file_dialog
            AppAction::UploadFile => {}

            AppAction::SaveToRemote => {
                if validation.has_errors(&editor) {
                    status.set("输入错误：请修正标红的数值后再保存");
                    continue;
                }
                let request = match save_request(&session, &editor) {
                    Ok(request) => request,
                    Err(error) => {
                        status.set(format!("保存失败: {error}"));
                        continue;
                    }
                };
                session.busy = BusyState::Working("正在保存".into());
                status.set("");
                session.send(request);
            }

            AppAction::FetchChassisPose => {
                let Some(target) = validated_pose_target(&editor, &mut status) else {
                    continue;
                };
                session.busy = BusyState::Fetching("底盘位姿".into());
                status.set("");
                session.pending_command = Some(PendingCommand::ChassisPose {
                    target,
                    structure_version: editor.structure_version,
                });
                let command = ros_cmd(
                    &session,
                    "timeout 15 ros2 topic echo /tracked_pose --once 2>/dev/null",
                );
                session.send(WorkerRequest::ExecCommand { command });
            }

            AppAction::FetchHeadJoints => {
                let Some(target) = validated_pose_target(&editor, &mut status) else {
                    continue;
                };
                session.busy = BusyState::Fetching("头部关节角".into());
                status.set("");
                session.pending_command = Some(PendingCommand::HeadJoints {
                    target,
                    structure_version: editor.structure_version,
                });
                let command = ros_cmd(&session, "timeout 15 python3 -");
                session.send(WorkerRequest::ExecCommandWithStdin {
                    command,
                    stdin_data: JOINT_STATES_SCRIPT.to_string(),
                });
            }

            AppAction::FetchWaistJoints => {
                let Some(target) = validated_pose_target(&editor, &mut status) else {
                    continue;
                };
                session.busy = BusyState::Fetching("腰部关节角".into());
                status.set("");
                session.pending_command = Some(PendingCommand::WaistJoints {
                    target,
                    structure_version: editor.structure_version,
                });
                let command = ros_cmd(&session, "timeout 15 python3 -");
                session.send(WorkerRequest::ExecCommandWithStdin {
                    command,
                    stdin_data: JOINT_STATES_SCRIPT.to_string(),
                });
            }

            AppAction::SelectPose(target) => {
                editor.selected_pose = Some(target.clone());
            }

            AppAction::CreatePose(path) => {
                if validation.has_errors(&editor) {
                    status.set("输入错误：请先修正标红数值，再创建位姿");
                    continue;
                }
                let created =
                    editor.data.as_mut().is_some_and(|data| {
                        match model::field_at_path_mut(&mut data.context_fields, path) {
                            Some(field) => {
                                field.value = ContextValue::Pose(RobotPose::default());
                                true
                            }
                            None => false,
                        }
                    });
                if created {
                    // 字段类型变了，控件树要重建
                    editor.mark_structure_changed();
                }
            }

            AppAction::ReloadSshHosts => {
                // 每次打开菜单都重新读取，编辑过 ssh config 后无需重启
                session.ssh_hosts = ssh_config::load_user_hosts();
                session.hosts_version += 1;
            }

            AppAction::ApplySshHost(index) => {
                apply_ssh_host(&mut session, &mut status, *index);
            }
        }
    }
}

/// 处理需要弹原生文件对话框的操作
///
/// **必须钉在主线程**：macOS 的 `NSOpenPanel` 只能在主线程调用，而 Bevy 的多线程
/// 调度器会把普通 system 丢到 Compute Task Pool（实测跑在 "Compute Task Pool (2)"），
/// 在那里弹对话框会崩溃或卡死。`NonSendMarker` 是 `!Send` 的空类型，
/// 带上它就会把这个 system 固定在主线程执行。
///
/// 对话框是模态的，打开期间主循环会停住、窗口不刷新——这与旧版行为一致。
fn handle_file_dialog(
    _main_thread: NonSendMarker,
    mut actions: MessageReader<DispatchAction>,
    mut session: ResMut<Session>,
    mut status: ResMut<StatusLine>,
    browser: Res<FileBrowser>,
) {
    for DispatchAction(action) in actions.read() {
        if !matches!(action, AppAction::UploadFile) {
            continue;
        }
        if !session.is_connected || !session.interactive() {
            continue;
        }
        let Some(remote_dir) = browser.remote_dir.clone() else {
            status.set("请先刷新要上传到的远程目录");
            continue;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_title("选择要上传的 JSON 文件")
            .pick_file()
        else {
            continue;
        };
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                session.busy = BusyState::Working(format!("正在上传 {filename}"));
                status.set("");
                session.send(WorkerRequest::UploadFile {
                    remote_dir,
                    filename,
                    content,
                });
            }
            Err(e) => status.set(format!("读取本地文件失败: {e}")),
        }
    }
}

/// 更新文件列表（从 worker 响应中提取）
fn apply_file_list(
    browser: &mut FileBrowser,
    status: &mut StatusLine,
    result: Result<DirListing, String>,
) {
    match result {
        Ok(listing) => {
            debug!(
                dir = %listing.resolved_dir,
                json = listing.json_files.len(),
                entries = listing.total_entries,
                "文件列表已更新"
            );
            if listing.json_files.is_empty() {
                status.set(empty_dir_hint(&listing));
            }
            if browser.remote_dir.as_deref() != Some(listing.resolved_dir.as_str()) {
                browser.selected = None;
            }
            browser.remote_dir = Some(listing.resolved_dir);
            if browser
                .selected
                .as_ref()
                .is_some_and(|name| !listing.json_files.contains(name))
            {
                browser.selected = None;
            }
            browser.set_files(listing.json_files);
        }
        Err(e) => {
            warn!(error = %e, "获取文件列表失败");
            status.set(format!("获取文件列表失败: {e}"));
        }
    }
}

/// 后台写操作只刷新它实际操作的目录，不能把用户已切换的列表切回旧目录。
fn apply_operation_file_list(
    browser: &mut FileBrowser,
    status: &mut StatusLine,
    result: Result<DirListing, String>,
) {
    match result {
        Ok(listing) if browser.remote_dir.as_deref() == Some(listing.resolved_dir.as_str()) => {
            apply_file_list(browser, status, Ok(listing));
        }
        Ok(_) => {}
        Err(error) => status.set(format!("操作已完成，但刷新目录失败: {error}")),
    }
}

/// 目录里没有 JSON 时给出可操作的提示
///
/// 光说"无 JSON 文件"看不出是路径写错、目录为空、还是文件在下一层，
/// 所以把实际扫描的绝对路径和子目录一并说清楚。
fn empty_dir_hint(listing: &DirListing) -> String {
    let dir = &listing.resolved_dir;
    match (listing.total_entries, listing.subdirs.as_slice()) {
        (0, _) => format!("注意：{dir} 是空目录"),
        (_, []) => format!(
            "注意：{dir} 下无 JSON 文件（共 {} 个条目）",
            listing.total_entries
        ),
        (_, subdirs) => {
            // 子目录可能很多，只列前几个够提示就行
            let shown: Vec<&str> = subdirs.iter().take(5).map(String::as_str).collect();
            let more = match subdirs.len() > shown.len() {
                true => format!(" 等 {} 个", subdirs.len()),
                false => String::new(),
            };
            format!(
                "注意：{dir} 下无 JSON 文件，但有子目录 {}{more}——要找的是不是在下一层？",
                shown.join("、")
            )
        }
    }
}

/// 响应类型名，只用于日志
fn response_kind(response: &WorkerResponse) -> &'static str {
    match response {
        WorkerResponse::Connected { .. } => "Connected",
        WorkerResponse::ConnectFailed(_) => "ConnectFailed",
        WorkerResponse::Disconnected => "Disconnected",
        WorkerResponse::FileList(_) => "FileList",
        WorkerResponse::FileLoaded { .. } => "FileLoaded",
        WorkerResponse::FileSaved { .. } => "FileSaved",
        WorkerResponse::SaveFailed(_) => "SaveFailed",
        WorkerResponse::BackupDone { .. } => "BackupDone",
        WorkerResponse::BackupFailed(_) => "BackupFailed",
        WorkerResponse::FileDeleted { .. } => "FileDeleted",
        WorkerResponse::DeleteFailed(_) => "DeleteFailed",
        WorkerResponse::FileUploaded { .. } => "FileUploaded",
        WorkerResponse::UploadFailed(_) => "UploadFailed",
        WorkerResponse::CommandOutput(_) => "CommandOutput",
        WorkerResponse::ConnectionLost(_) => "ConnectionLost",
        WorkerResponse::Reconnecting { .. } => "Reconnecting",
        WorkerResponse::Reconnected { .. } => "Reconnected",
    }
}

/// 处理一条后台线程返回的响应
fn handle_response(
    response: WorkerResponse,
    session: &mut Session,
    status: &mut StatusLine,
    browser: &mut FileBrowser,
    editor: &mut Editor,
) {
    match response {
        WorkerResponse::Connected {
            ros_domain_id,
            file_list,
        } => {
            session.busy = BusyState::Idle;
            session.is_connected = true;
            session.update_ros_domain_id(ros_domain_id);
            let target = session
                .target
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            status.set(format!("已连接到 {target}"));
            session.save_login();
            // 连接成功但列不出文件（多半是远程目录写错），错误要盖过连接成功的提示
            apply_file_list(browser, status, file_list);
        }

        WorkerResponse::ConnectFailed(e) => {
            session.busy = BusyState::Idle;
            session.is_connected = false;
            session.reconnect_status = None;
            status.set(format!("连接失败: {e}"));
        }

        WorkerResponse::Disconnected => {
            session.busy = BusyState::Idle;
        }

        WorkerResponse::FileList(result) => {
            session.busy = BusyState::Idle;
            apply_file_list(browser, status, result);
        }

        WorkerResponse::FileLoaded {
            remote_dir,
            filename,
            result,
        } => {
            session.busy = BusyState::Idle;
            match result {
                Ok(content) => match model::parse_task_graph(&content) {
                    Ok(data) => {
                        editor.load_remote(
                            data,
                            RemoteDocument {
                                connection_generation: session.connection_generation,
                                remote_dir,
                                filename: filename.clone(),
                            },
                        );
                        status.set(format!("已加载: {filename}"));
                    }
                    Err(e) => {
                        status.set(format!("解析失败: {e}"));
                        editor.load(None);
                    }
                },
                Err(e) => {
                    status.set(format!("读取失败: {e}"));
                    editor.load(None);
                }
            }
        }

        WorkerResponse::FileSaved {
            remote_dir,
            old_filename,
            new_filename,
            cleanup_warning,
            file_list,
        } => {
            session.busy = BusyState::Idle;
            match &new_filename {
                Some(new_name) => {
                    status.set(format!("已更新并重命名: {old_filename} → {new_name}"));
                    if browser.remote_dir.as_deref() == Some(remote_dir.as_str()) {
                        browser.selected = Some(new_name.clone());
                    }
                    if let Some(document) = &mut editor.document
                        && document.remote_dir == remote_dir
                        && document.filename == old_filename
                    {
                        document.filename.clone_from(new_name);
                    }
                }
                None => status.set(format!("已更新远程文件: {old_filename}")),
            }
            apply_operation_file_list(browser, status, file_list);
            if let Some(warning) = cleanup_warning {
                status.set(format!("文件已保存；警告：{warning}"));
            }
        }

        WorkerResponse::SaveFailed(e) => {
            session.busy = BusyState::Idle;
            status.set(format!("保存失败: {e}"));
        }

        WorkerResponse::BackupDone {
            original,
            backup_name,
            file_list,
        } => {
            session.busy = BusyState::Idle;
            status.set(format!("已备份: {original} → {backup_name}"));
            apply_operation_file_list(browser, status, file_list);
        }

        WorkerResponse::BackupFailed(e) => {
            session.busy = BusyState::Idle;
            status.set(format!("备份失败: {e}"));
        }

        WorkerResponse::FileDeleted {
            remote_dir,
            filename,
            file_list,
        } => {
            session.busy = BusyState::Idle;
            status.set(format!("已删除: {filename}"));
            if browser.remote_dir.as_deref() == Some(remote_dir.as_str())
                && browser.selected.as_deref() == Some(filename.as_str())
            {
                browser.selected = None;
            }
            if editor.document.as_ref().is_some_and(|document| {
                document.remote_dir == remote_dir && document.filename == filename
            }) {
                editor.load(None);
            }
            apply_operation_file_list(browser, status, file_list);
        }

        WorkerResponse::DeleteFailed(e) => {
            session.busy = BusyState::Idle;
            status.set(format!("删除失败: {e}"));
        }

        WorkerResponse::FileUploaded {
            filename,
            file_list,
        } => {
            session.busy = BusyState::Idle;
            status.set(format!("已上传: {filename}"));
            apply_operation_file_list(browser, status, file_list);
        }

        WorkerResponse::UploadFailed(e) => {
            session.busy = BusyState::Idle;
            status.set(format!("上传失败: {e}"));
        }

        WorkerResponse::ConnectionLost(reason) => {
            session.is_connected = false;
            session.busy = BusyState::Idle;
            session.pending_command = None;
            status.set(format!("连接已断开: {reason}"));
        }

        WorkerResponse::Reconnecting {
            attempt,
            delay_secs,
        } => {
            session.reconnect_status = Some(match attempt {
                0 => format!("{delay_secs}秒后自动重连..."),
                n => format!("第{n}次重连失败，{delay_secs}秒后重试..."),
            });
        }

        WorkerResponse::Reconnected {
            ros_domain_id,
            file_list,
        } => {
            session.is_connected = true;
            session.reconnect_status = None;
            session.update_ros_domain_id(ros_domain_id);
            let target = session
                .target
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            status.set(format!("已重新连接到 {target}"));
            apply_file_list(browser, status, file_list);
        }

        WorkerResponse::CommandOutput(result) => {
            session.busy = BusyState::Idle;
            let pending = session.pending_command.take();
            handle_command_output(result, pending, status, editor);
        }
    }
}

/// 处理远程命令输出：按等待中的命令类型解析并回填位姿
fn handle_command_output(
    result: Result<String, String>,
    pending: Option<PendingCommand>,
    status: &mut StatusLine,
    editor: &mut Editor,
) {
    let Some(pending) = pending else {
        // 没有对应请求的输出不能回填当前选中。
        return;
    };
    let structure_version = match &pending {
        PendingCommand::ChassisPose {
            structure_version, ..
        }
        | PendingCommand::HeadJoints {
            structure_version, ..
        }
        | PendingCommand::WaistJoints {
            structure_version, ..
        } => *structure_version,
    };
    if structure_version != editor.structure_version {
        status.set("文档或字段结构已变化，已忽略过期的位姿获取结果");
        return;
    }
    let output = match result {
        Ok(output) => output,
        Err(e) => {
            status.set(format!("远程命令执行失败: {e}"));
            return;
        }
    };

    // 解析输出并写入位姿，返回 (提示前缀, 写入结果)
    let (label, applied) = match pending {
        PendingCommand::ChassisPose { target, .. } => {
            let Some(chassis) = model::parse_tracked_pose(&output) else {
                status.set("解析 tracked_pose 输出失败");
                return;
            };
            (
                "底盘位姿",
                apply_to_pose_at(editor, &target, |pose| pose.chassis_pose = chassis),
            )
        }
        PendingCommand::HeadJoints { target, .. } => {
            let Some(angles) = model::parse_joint_states(&output) else {
                status.set("解析 joint_states 输出失败");
                return;
            };
            (
                "头部关节角",
                apply_to_pose_at(editor, &target, |pose| {
                    pose.head_pose.position.x = angles.head_joint_1;
                    pose.head_pose.position.y = angles.head_joint_2;
                }),
            )
        }
        PendingCommand::WaistJoints { target, .. } => {
            let Some(angles) = model::parse_joint_states(&output) else {
                status.set("解析 joint_states 输出失败");
                return;
            };
            (
                "腰部关节角",
                apply_to_pose_at(editor, &target, |pose| {
                    pose.waist_pose.position.x = angles.body_joint_1;
                    pose.waist_pose.position.y = angles.body_joint_2;
                }),
            )
        }
    };

    match applied {
        Some(key_path) => {
            // 数据被 UI 之外的来源改了，已有控件要刷新显示
            editor.mark_values_changed();
            status.set(format!("已填入{label} → {key_path}"));
        }
        None => status.set("目标位姿字段已不存在"),
    }
}

/// 轮询后台线程响应
fn poll_worker(
    mut session: ResMut<Session>,
    mut status: ResMut<StatusLine>,
    mut browser: ResMut<FileBrowser>,
    mut editor: ResMut<Editor>,
) {
    let responses: Vec<_> = session
        .worker
        .as_ref()
        .map(|w| std::iter::from_fn(|| w.try_recv()).collect())
        .unwrap_or_default();

    for response in responses {
        debug!(kind = response_kind(&response), "收到后台响应");
        handle_response(
            response,
            &mut session,
            &mut status,
            &mut browser,
            &mut editor,
        );
    }
}

/// 桥接插件
pub struct WorkerBridgePlugin;

/// 将本帧输入应用到文本缓冲、数据绑定和密码状态，再执行界面动作。
///
/// Bevy 默认在 PostUpdate 才应用文本编辑，而保存/连接在 Update 处理。
/// 提前刷新一次可避免同帧输入加点击提交旧值；PostUpdate 仍负责后续重建产生的编辑。
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct InputFlush;

#[cfg(test)]
mod tests;

impl Plugin for WorkerBridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<AppAction>()
            .add_message::<DispatchAction>()
            .init_resource::<PendingActions>()
            .configure_sets(Update, InputFlush.after(UiSet::Input).before(UiSet::Update))
            .add_systems(
                Update,
                (
                    bevy::text::apply_text_edits,
                    super::connect::sync_password,
                    dispatch_after_text_input,
                )
                    .chain()
                    .in_set(InputFlush),
            )
            .add_systems(
                Update,
                (poll_worker, handle_actions, handle_file_dialog)
                    .chain()
                    .in_set(UiSet::Update),
            );
    }
}

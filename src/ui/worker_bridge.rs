//! UI 与后台 SSH 线程之间的桥接
//!
//! 三件事：
//! 1. 把界面上的操作意图收敛成 [`AppAction`] 消息，统一在一处翻译成 `WorkerRequest`；
//! 2. 每帧轮询后台线程的响应，更新状态资源；
//! 3. 提供跨线程唤醒回调——后台线程有响应时通过 winit 的 `EventLoopProxy`
//!    把主循环从"无输入不重绘"的休眠中叫醒。

use std::sync::Arc;

use bevy::prelude::*;
use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};

use crate::model::{self, ContextValue, RobotPose};
use crate::ssh::{AuthMethod, DirListing};
use crate::ssh_config;
use crate::worker::{BusyState, WakeFn, WorkerHandle, WorkerRequest, WorkerResponse};

use super::{Editor, FileBrowser, PendingCommand, Session, StatusLine, UiSet};

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
    /// 选中某个位姿字段（索引路径）
    SelectPose(Vec<usize>),
    /// 在 null 字段上创建一个默认位姿
    CreatePose(Vec<usize>),
    /// 重新读取 `~/.ssh/config`
    ReloadSshHosts,
    /// 套用 ssh config 中的第 n 个主机条目
    ApplySshHost(usize),
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

/// 校验当前选中位姿并返回其索引路径（无有效选中时写状态提示）
fn validated_pose_path(editor: &Editor, status: &mut StatusLine) -> Option<Vec<usize>> {
    let Some(path) = editor.selected_pose_path.clone() else {
        status.set("请先选中一个位姿点位");
        return None;
    };
    if !editor.has_pose_selection() {
        status.set("选中的字段不是位姿类型");
        return None;
    }
    Some(path)
}

/// 把 ssh config 中的主机条目填入连接表单（密码保持不动，由用户自行决定）
fn apply_ssh_host(session: &mut Session, status: &mut StatusLine, index: usize) {
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

/// 将获取到的远程数据写入指定路径的位姿字段，成功时返回 key 路径（用于状态提示）
fn apply_to_pose_at(
    editor: &mut Editor,
    field_path: &[usize],
    apply: impl FnOnce(&mut RobotPose),
) -> Option<String> {
    let data = editor.data.as_mut()?;
    let field = model::field_at_path_mut(&mut data.context_fields, field_path)?;
    let ContextValue::Pose(pose) = &mut field.value else {
        return None;
    };
    apply(pose);
    Some(model::key_path_string(&data.context_fields, field_path))
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

/// 处理界面操作意图
fn handle_actions(
    mut actions: MessageReader<AppAction>,
    mut session: ResMut<Session>,
    mut status: ResMut<StatusLine>,
    mut browser: ResMut<FileBrowser>,
    mut editor: ResMut<Editor>,
    proxy: Res<EventLoopProxyWrapper>,
) {
    for action in actions.read() {
        debug!(?action, "处理界面操作");
        match action {
            AppAction::Connect => {
                let port = session.login.port.parse::<u16>().unwrap_or(22);
                // 每次连接创建新的 worker 线程
                let worker = WorkerHandle::spawn(make_wake_fn(&proxy));
                worker.send(WorkerRequest::Connect {
                    host: session.login.host.clone(),
                    port,
                    username: session.login.username.clone(),
                    auth: auth_method(&session),
                    remote_dir: session.login.remote_dir.clone(),
                });
                session.worker = Some(worker);
                session.busy = BusyState::Connecting;
                status.set("正在连接...");
            }

            AppAction::Disconnect => {
                session.send(WorkerRequest::Disconnect);
                session.worker = None;
                session.is_connected = false;
                session.reconnect_status = None;
                session.pending_command = None;
                session.busy = BusyState::Idle;
                browser.set_files(Vec::new());
                browser.selected = None;
                editor.load(None);
                status.set("已断开连接");
            }

            AppAction::RefreshFiles => {
                session.busy = BusyState::Refreshing;
                session.send(WorkerRequest::RefreshFiles {
                    remote_dir: session.login.remote_dir.clone(),
                });
            }

            AppAction::LoadFile(filename) => {
                session.busy = BusyState::Loading(filename.clone());
                status.set(format!("正在加载: {filename}"));
                browser.selected = Some(filename.clone());
                session.send(WorkerRequest::LoadFile {
                    remote_dir: session.login.remote_dir.clone(),
                    filename: filename.clone(),
                });
            }

            AppAction::BackupFile(filename) => {
                session.busy = BusyState::Saving;
                status.set(format!("正在备份: {filename}"));
                session.send(WorkerRequest::BackupFile {
                    remote_dir: session.login.remote_dir.clone(),
                    filename: filename.clone(),
                    existing_files: browser.files.clone(),
                });
            }

            AppAction::DeleteFile(filename) => {
                session.busy = BusyState::Saving;
                status.set(format!("正在删除: {filename}"));
                session.send(WorkerRequest::DeleteFile {
                    remote_dir: session.login.remote_dir.clone(),
                    filename: filename.clone(),
                });
            }

            AppAction::UploadFile => {
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
                        session.busy = BusyState::Saving;
                        status.set(format!("正在上传: {filename}"));
                        session.send(WorkerRequest::UploadFile {
                            remote_dir: session.login.remote_dir.clone(),
                            filename,
                            content,
                        });
                    }
                    Err(e) => status.set(format!("读取本地文件失败: {e}")),
                }
            }

            AppAction::SaveToRemote => {
                let Some(current_filename) = browser.selected.clone() else {
                    status.set("未选中文件");
                    continue;
                };
                let Some(data) = &editor.data else {
                    status.set("无数据可保存");
                    continue;
                };
                let content = match model::serialize_task_graph(data) {
                    Ok(c) => c,
                    Err(e) => {
                        status.set(format!("序列化失败: {e}"));
                        continue;
                    }
                };
                // task_id 改过就顺带把远程文件改名
                let new_name = format!("{}.json", data.task_id);
                let new_filename = (new_name != current_filename).then_some(new_name);

                session.busy = BusyState::Saving;
                status.set("正在保存...");
                session.send(WorkerRequest::SaveFile {
                    remote_dir: session.login.remote_dir.clone(),
                    current_filename,
                    content,
                    new_filename,
                });
            }

            AppAction::FetchChassisPose => {
                let Some(path) = validated_pose_path(&editor, &mut status) else {
                    continue;
                };
                session.busy = BusyState::Fetching("底盘位姿".into());
                status.set("正在获取底盘位姿...");
                session.pending_command = Some(PendingCommand::ChassisPose { field_path: path });
                let command = ros_cmd(
                    &session,
                    "timeout 15 ros2 topic echo /tracked_pose --once 2>/dev/null",
                );
                session.send(WorkerRequest::ExecCommand { command });
            }

            AppAction::FetchHeadJoints => {
                let Some(path) = validated_pose_path(&editor, &mut status) else {
                    continue;
                };
                session.busy = BusyState::Fetching("头部关节角".into());
                status.set("正在获取头部关节角...");
                session.pending_command = Some(PendingCommand::HeadJoints { field_path: path });
                let command = ros_cmd(&session, "timeout 15 python3 -");
                session.send(WorkerRequest::ExecCommandWithStdin {
                    command,
                    stdin_data: JOINT_STATES_SCRIPT.to_string(),
                });
            }

            AppAction::FetchWaistJoints => {
                let Some(path) = validated_pose_path(&editor, &mut status) else {
                    continue;
                };
                session.busy = BusyState::Fetching("腰部关节角".into());
                status.set("正在获取腰部关节角...");
                session.pending_command = Some(PendingCommand::WaistJoints { field_path: path });
                let command = ros_cmd(&session, "timeout 15 python3 -");
                session.send(WorkerRequest::ExecCommandWithStdin {
                    command,
                    stdin_data: JOINT_STATES_SCRIPT.to_string(),
                });
            }

            AppAction::SelectPose(path) => {
                editor.selected_pose_path = Some(path.clone());
            }

            AppAction::CreatePose(path) => {
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
            browser.set_files(listing.json_files);
        }
        Err(e) => {
            warn!(error = %e, "获取文件列表失败");
            status.set(format!("获取文件列表失败: {e}"));
        }
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
            if let Some(id) = ros_domain_id {
                session.login.ros_domain_id = id;
            }
            status.set(format!("已连接到 {}", session.login.host));
            session.save_login();
            // 连接成功但列不出文件（多半是远程目录写错），错误要盖过连接成功的提示
            apply_file_list(browser, status, file_list);
        }

        WorkerResponse::ConnectFailed(e) => {
            session.busy = BusyState::Idle;
            session.is_connected = false;
            status.set(format!("连接失败: {e}"));
        }

        WorkerResponse::Disconnected => {
            session.busy = BusyState::Idle;
        }

        WorkerResponse::FileList(result) => {
            session.busy = BusyState::Idle;
            apply_file_list(browser, status, result);
        }

        WorkerResponse::FileLoaded { filename, result } => {
            session.busy = BusyState::Idle;
            match result {
                Ok(content) => match model::parse_task_graph(&content) {
                    Ok(data) => {
                        editor.load(Some(data));
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
            old_filename,
            new_filename,
            file_list,
        } => {
            session.busy = BusyState::Idle;
            match &new_filename {
                Some(new_name) => {
                    status.set(format!("已更新并重命名: {old_filename} → {new_name}"));
                    browser.selected = Some(new_name.clone());
                }
                None => status.set(format!("已更新远程文件: {old_filename}")),
            }
            apply_file_list(browser, status, file_list);
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
            apply_file_list(browser, status, file_list);
        }

        WorkerResponse::BackupFailed(e) => {
            session.busy = BusyState::Idle;
            status.set(format!("备份失败: {e}"));
        }

        WorkerResponse::FileDeleted {
            filename,
            file_list,
        } => {
            session.busy = BusyState::Idle;
            status.set(format!("已删除: {filename}"));
            if browser.selected.as_deref() == Some(filename.as_str()) {
                browser.selected = None;
                editor.load(None);
            }
            apply_file_list(browser, status, file_list);
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
            apply_file_list(browser, status, file_list);
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
            if let Some(id) = ros_domain_id {
                session.login.ros_domain_id = id;
            }
            status.set(format!("已重新连接到 {}", session.login.host));
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
    let output = match (result, &pending) {
        (Ok(output), Some(_)) => output,
        (Err(e), Some(_)) => {
            status.set(format!("远程命令执行失败: {e}"));
            return;
        }
        // 没有 pending_command 的 CommandOutput，忽略
        (_, None) => return,
    };

    // 解析输出并写入位姿，返回 (提示前缀, 写入结果)
    let (label, applied) = match pending {
        Some(PendingCommand::ChassisPose { field_path }) => {
            let Some(chassis) = model::parse_tracked_pose(&output) else {
                status.set("解析 tracked_pose 输出失败");
                return;
            };
            (
                "底盘位姿",
                apply_to_pose_at(editor, &field_path, |pose| pose.chassis_pose = chassis),
            )
        }
        Some(PendingCommand::HeadJoints { field_path }) => {
            let Some(angles) = model::parse_joint_states(&output) else {
                status.set("解析 joint_states 输出失败");
                return;
            };
            (
                "头部关节角",
                apply_to_pose_at(editor, &field_path, |pose| {
                    pose.head_pose.position.x = angles.head_joint_1;
                    pose.head_pose.position.y = angles.head_joint_2;
                }),
            )
        }
        Some(PendingCommand::WaistJoints { field_path }) => {
            let Some(angles) = model::parse_joint_states(&output) else {
                status.set("解析 joint_states 输出失败");
                return;
            };
            (
                "腰部关节角",
                apply_to_pose_at(editor, &field_path, |pose| {
                    pose.waist_pose.position.x = angles.body_joint_1;
                    pose.waist_pose.position.y = angles.body_joint_2;
                }),
            )
        }
        None => return,
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

impl Plugin for WorkerBridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<AppAction>().add_systems(
            Update,
            (poll_worker, handle_actions).chain().in_set(UiSet::Update),
        );
    }
}

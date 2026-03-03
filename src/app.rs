//! GUI 应用主模块 - eframe/egui 界面实现

use eframe::egui;

use crate::model::{self, LoginConfig, Pose, TaskGraphData};
use crate::worker::{BusyState, WorkerHandle, WorkerRequest, WorkerResponse};

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

/// 格式化 f64，保留完整精度，至少显示一位小数
fn format_f64(v: f64, _decimals: std::ops::RangeInclusive<usize>) -> String {
    let s = format!("{v}");
    if s.contains('.') { s } else { format!("{v}.0") }
}

/// 等待中的远程命令类型（用于识别 CommandOutput 响应的来源）
enum PendingCommand {
    /// 获取底盘位姿
    ChassisPose { pose_index: usize },
    /// 获取头部关节角
    HeadJoints { pose_index: usize },
    /// 获取腰部关节角
    WaistJoints { pose_index: usize },
}

/// 应用主状态
pub struct App {
    // SSH 连接参数
    host: String,
    port: String,
    username: String,
    password: String,

    /// ROS_DOMAIN_ID（不同机器人可能不同）
    ros_domain_id: String,

    /// 远程任务图文件目录
    remote_dir: String,

    /// UI 缩放倍率
    ui_scale: f32,

    // 后台工作线程句柄
    worker: Option<WorkerHandle>,

    /// 当前忙碌状态
    busy: BusyState,

    /// 是否已连接（由 worker 响应驱动）
    is_connected: bool,

    status_message: String,

    // 文件列表
    file_list: Vec<String>,
    selected_file: Option<String>,

    // 编辑数据
    task_data: Option<TaskGraphData>,

    // 当前选中的位姿索引（用于高亮和获取底盘位姿）
    selected_pose_index: Option<usize>,

    /// 等待中的远程命令（用于解析 CommandOutput 响应）
    pending_command: Option<PendingCommand>,
}

impl Default for App {
    fn default() -> Self {
        // 从本地配置加载上次的登录信息
        let config = model::load_login_config();
        Self {
            host: config.host,
            port: config.port,
            username: config.username,
            password: config.password,
            ros_domain_id: config.ros_domain_id,
            remote_dir: config.remote_dir,
            ui_scale: 1.0,
            worker: None,
            busy: BusyState::Idle,
            is_connected: false,
            status_message: String::new(),
            file_list: Vec::new(),
            selected_file: None,
            task_data: None,
            selected_pose_index: None,
            pending_command: None,
        }
    }
}

impl App {
    /// 判断当前是否忙碌（有请求正在后台执行）
    fn is_busy(&self) -> bool {
        !matches!(self.busy, BusyState::Idle)
    }

    /// 获取忙碌状态描述文字（用于 UI 显示）
    fn busy_text(&self) -> &str {
        match &self.busy {
            BusyState::Idle => "",
            BusyState::Connecting => "正在连接...",
            BusyState::Refreshing => "正在刷新文件列表...",
            BusyState::Loading(name) => {
                // 返回静态文字，具体文件名在调用处拼接
                let _ = name;
                "正在加载文件..."
            }
            BusyState::Saving => "正在保存...",
            BusyState::Fetching(desc) => {
                let _ = desc;
                "正在获取数据..."
            }
        }
    }

    /// 发送请求到后台线程
    fn send_request(&self, request: WorkerRequest) {
        if let Some(worker) = &self.worker {
            worker.send(request);
        }
    }

    /// 构建远程 ROS2 命令（自动加 source 环境 + ROS_DOMAIN_ID）
    fn ros_cmd(&self, cmd: &str) -> String {
        format!(
            "bash -c 'export ROS_DOMAIN_ID={} && {ROS_ENV_PREFIX} && {cmd}'",
            self.ros_domain_id
        )
    }

    /// 保存当前登录信息到本地配置文件
    fn save_login_config(&self) {
        let config = LoginConfig {
            host: self.host.clone(),
            port: self.port.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            ros_domain_id: self.ros_domain_id.clone(),
            remote_dir: self.remote_dir.clone(),
        };
        model::save_login_config(&config);
    }

    /// 尝试连接远程主机（异步）
    fn connect(&mut self, ctx: &egui::Context) {
        let port = self.port.parse::<u16>().unwrap_or(22);

        // 每次连接创建新的 worker 线程
        let worker = WorkerHandle::spawn(ctx.clone());
        worker.send(WorkerRequest::Connect {
            host: self.host.clone(),
            port,
            username: self.username.clone(),
            password: self.password.clone(),
            remote_dir: self.remote_dir.clone(),
        });
        self.worker = Some(worker);
        self.busy = BusyState::Connecting;
        self.status_message = "正在连接...".into();
    }

    /// 断开连接
    fn disconnect(&mut self) {
        if let Some(worker) = &self.worker {
            worker.send(WorkerRequest::Disconnect);
        }
        // Drop WorkerHandle，后台线程的 rx 会断开，线程自然退出
        self.worker = None;
        self.is_connected = false;
        self.file_list.clear();
        self.selected_file = None;
        self.task_data = None;
        self.selected_pose_index = None;
        self.pending_command = None;
        self.busy = BusyState::Idle;
        self.status_message = "已断开连接".into();
    }

    /// 刷新远程文件列表（异步）
    fn refresh_file_list(&mut self) {
        self.busy = BusyState::Refreshing;
        self.send_request(WorkerRequest::RefreshFiles {
            remote_dir: self.remote_dir.clone(),
        });
    }

    /// 加载选中的文件（异步）
    fn load_file(&mut self, filename: &str) {
        self.busy = BusyState::Loading(filename.to_string());
        self.status_message = format!("正在加载: {filename}");
        self.send_request(WorkerRequest::LoadFile {
            remote_dir: self.remote_dir.clone(),
            filename: filename.to_string(),
        });
    }

    /// 将编辑后的数据写回远程文件（异步）
    fn save_to_remote(&mut self) {
        let Some(current_filename) = self.selected_file.clone() else {
            self.status_message = "未选中文件".into();
            return;
        };
        let Some(data) = &self.task_data else {
            self.status_message = "无数据可保存".into();
            return;
        };

        let content = match model::serialize_task_graph(data) {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("序列化失败: {e}");
                return;
            }
        };

        let new_filename = {
            let new_name = format!("{}.json", data.task_id);
            if new_name != current_filename {
                Some(new_name)
            } else {
                None
            }
        };

        self.busy = BusyState::Saving;
        self.status_message = "正在保存...".into();

        self.send_request(WorkerRequest::SaveFile {
            remote_dir: self.remote_dir.clone(),
            current_filename,
            content,
            new_filename,
        });
    }

    /// 获取底盘位姿（异步）
    fn fetch_chassis_pose(&mut self) {
        let Some(idx) = self.selected_pose_index else {
            self.status_message = "请先选中一个位姿点位".into();
            return;
        };

        self.busy = BusyState::Fetching("底盘位姿".into());
        self.status_message = "正在获取底盘位姿...".into();
        self.pending_command = Some(PendingCommand::ChassisPose { pose_index: idx });

        let cmd = self.ros_cmd("timeout 15 ros2 topic echo /tracked_pose --once 2>/dev/null");
        self.send_request(WorkerRequest::ExecCommand { command: cmd });
    }

    /// 获取头部关节角（异步）
    fn fetch_head_joints(&mut self) {
        let Some(idx) = self.selected_pose_index else {
            self.status_message = "请先选中一个位姿点位".into();
            return;
        };

        self.busy = BusyState::Fetching("头部关节角".into());
        self.status_message = "正在获取头部关节角...".into();
        self.pending_command = Some(PendingCommand::HeadJoints { pose_index: idx });

        let cmd = self.ros_cmd("timeout 15 python3 -");
        self.send_request(WorkerRequest::ExecCommandWithStdin {
            command: cmd,
            stdin_data: JOINT_STATES_SCRIPT.to_string(),
        });
    }

    /// 获取腰部关节角（异步）
    fn fetch_waist_joints(&mut self) {
        let Some(idx) = self.selected_pose_index else {
            self.status_message = "请先选中一个位姿点位".into();
            return;
        };

        self.busy = BusyState::Fetching("腰部关节角".into());
        self.status_message = "正在获取腰部关节角...".into();
        self.pending_command = Some(PendingCommand::WaistJoints { pose_index: idx });

        let cmd = self.ros_cmd("timeout 15 python3 -");
        self.send_request(WorkerRequest::ExecCommandWithStdin {
            command: cmd,
            stdin_data: JOINT_STATES_SCRIPT.to_string(),
        });
    }

    /// 备份远程文件（异步）
    fn backup_file(&mut self, filename: &str) {
        self.busy = BusyState::Saving;
        self.status_message = format!("正在备份: {filename}");

        self.send_request(WorkerRequest::BackupFile {
            remote_dir: self.remote_dir.clone(),
            filename: filename.to_string(),
            existing_files: self.file_list.clone(),
        });
    }

    /// 删除远程文件（异步）
    fn delete_file(&mut self, filename: &str) {
        self.busy = BusyState::Saving;
        self.status_message = format!("正在删除: {filename}");

        self.send_request(WorkerRequest::DeleteFile {
            remote_dir: self.remote_dir.clone(),
            filename: filename.to_string(),
        });
    }

    /// 弹出本地文件选择对话框，将选中的文件上传到远程目录（对话框同步，上传异步）
    fn upload_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_title("选择要上传的 JSON 文件")
            .pick_file()
        else {
            return; // 用户取消了对话框
        };

        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        match std::fs::read_to_string(&path) {
            Ok(content) => {
                self.busy = BusyState::Saving;
                self.status_message = format!("正在上传: {filename}");

                self.send_request(WorkerRequest::UploadFile {
                    remote_dir: self.remote_dir.clone(),
                    filename,
                    content,
                });
            }
            Err(e) => {
                self.status_message = format!("读取本地文件失败: {e}");
            }
        }
    }

    /// 更新文件列表（从 worker 响应中提取）
    fn apply_file_list(&mut self, result: Result<Vec<String>, String>) {
        match result {
            Ok(files) => {
                self.file_list = files;
                if self.file_list.is_empty() {
                    self.status_message = "远程目录下无 JSON 文件".into();
                }
            }
            Err(e) => {
                self.status_message = format!("获取文件列表失败: {e}");
            }
        }
    }

    /// 处理后台线程返回的响应
    fn handle_response(&mut self, response: WorkerResponse) {
        match response {
            WorkerResponse::Connected {
                ros_domain_id,
                file_list,
            } => {
                self.busy = BusyState::Idle;
                self.is_connected = true;
                if let Some(id) = ros_domain_id {
                    self.ros_domain_id = id;
                }
                self.file_list = file_list;
                self.status_message = format!("已连接到 {}", self.host);
                // 连接成功后保存登录信息
                self.save_login_config();
            }

            WorkerResponse::ConnectFailed(e) => {
                self.busy = BusyState::Idle;
                self.is_connected = false;
                self.status_message = format!("连接失败: {e}");
            }

            WorkerResponse::Disconnected => {
                // disconnect() 已经处理了 UI 状态，这里只确认
                self.busy = BusyState::Idle;
            }

            WorkerResponse::FileList(result) => {
                self.busy = BusyState::Idle;
                self.apply_file_list(result);
            }

            WorkerResponse::FileLoaded { filename, result } => {
                self.busy = BusyState::Idle;
                match result {
                    Ok(content) => match model::parse_task_graph(&content) {
                        Ok(data) => {
                            self.task_data = Some(data);
                            self.selected_pose_index = None;
                            self.status_message = format!("已加载: {filename}");
                        }
                        Err(e) => {
                            self.status_message = format!("解析失败: {e}");
                            self.task_data = None;
                        }
                    },
                    Err(e) => {
                        self.status_message = format!("读取失败: {e}");
                        self.task_data = None;
                    }
                }
            }

            WorkerResponse::FileSaved {
                old_filename,
                new_filename,
                file_list,
            } => {
                self.busy = BusyState::Idle;
                if let Some(new_name) = &new_filename {
                    self.status_message = format!("已更新并重命名: {old_filename} → {new_name}");
                    self.selected_file = Some(new_name.clone());
                } else {
                    self.status_message = format!("已更新远程文件: {old_filename}");
                }
                self.apply_file_list(file_list);
            }

            WorkerResponse::SaveFailed(e) => {
                self.busy = BusyState::Idle;
                self.status_message = format!("保存失败: {e}");
            }

            WorkerResponse::BackupDone {
                original,
                backup_name,
                file_list,
            } => {
                self.busy = BusyState::Idle;
                self.status_message = format!("已备份: {original} → {backup_name}");
                self.apply_file_list(file_list);
            }

            WorkerResponse::BackupFailed(e) => {
                self.busy = BusyState::Idle;
                self.status_message = format!("备份失败: {e}");
            }

            WorkerResponse::FileDeleted {
                filename,
                file_list,
            } => {
                self.busy = BusyState::Idle;
                self.status_message = format!("已删除: {filename}");
                // 如果删除的是当前选中文件，清空编辑区
                if self.selected_file.as_deref() == Some(filename.as_str()) {
                    self.selected_file = None;
                    self.task_data = None;
                    self.selected_pose_index = None;
                }
                self.apply_file_list(file_list);
            }

            WorkerResponse::DeleteFailed(e) => {
                self.busy = BusyState::Idle;
                self.status_message = format!("删除失败: {e}");
            }

            WorkerResponse::FileUploaded {
                filename,
                file_list,
            } => {
                self.busy = BusyState::Idle;
                self.status_message = format!("已上传: {filename}");
                self.apply_file_list(file_list);
            }

            WorkerResponse::UploadFailed(e) => {
                self.busy = BusyState::Idle;
                self.status_message = format!("上传失败: {e}");
            }

            WorkerResponse::CommandOutput(result) => {
                self.busy = BusyState::Idle;
                let pending = self.pending_command.take();
                match (result, pending) {
                    (Ok(output), Some(PendingCommand::ChassisPose { pose_index })) => {
                        match model::parse_tracked_pose(&output) {
                            Some(pose) => {
                                if let Some(data) = &mut self.task_data
                                    && let Some(field) = data.pose_fields.get_mut(pose_index)
                                {
                                    field.pose.chassis_pose = pose;
                                    self.status_message = format!("已填入底盘位姿 → {}", field.key);
                                }
                            }
                            None => {
                                self.status_message = "解析 tracked_pose 输出失败".into();
                            }
                        }
                    }
                    (Ok(output), Some(PendingCommand::HeadJoints { pose_index })) => {
                        match model::parse_joint_states(&output) {
                            Some(angles) => {
                                if let Some(data) = &mut self.task_data
                                    && let Some(field) = data.pose_fields.get_mut(pose_index)
                                {
                                    field.pose.head_pose.position.x = angles.head_joint_1;
                                    field.pose.head_pose.position.y = angles.head_joint_2;
                                    self.status_message =
                                        format!("已填入头部关节角 → {}", field.key);
                                }
                            }
                            None => {
                                self.status_message = "解析 joint_states 输出失败".into();
                            }
                        }
                    }
                    (Ok(output), Some(PendingCommand::WaistJoints { pose_index })) => {
                        match model::parse_joint_states(&output) {
                            Some(angles) => {
                                if let Some(data) = &mut self.task_data
                                    && let Some(field) = data.pose_fields.get_mut(pose_index)
                                {
                                    field.pose.waist_pose.position.x = angles.body_joint_1;
                                    field.pose.waist_pose.position.y = angles.body_joint_2;
                                    self.status_message =
                                        format!("已填入腰部关节角 → {}", field.key);
                                }
                            }
                            None => {
                                self.status_message = "解析 joint_states 输出失败".into();
                            }
                        }
                    }
                    (Err(e), Some(_)) => {
                        self.status_message = format!("远程命令执行失败: {e}");
                    }
                    (_, None) => {
                        // 没有 pending_command 的 CommandOutput，忽略
                    }
                }
            }
        }
    }

    /// 轮询后台线程响应（在 update() 开头调用）
    fn poll_worker(&mut self) {
        // 先收集所有响应，避免 &self.worker 和 &mut self 的借用冲突
        let responses: Vec<_> = self
            .worker
            .as_ref()
            .map(|w| std::iter::from_fn(|| w.try_recv()).collect())
            .unwrap_or_default();

        for response in responses {
            self.handle_response(response);
        }
    }

    /// 绘制左侧面板：连接表单 + 文件列表
    fn left_panel(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.heading("SSH 连接");
        ui.separator();

        let busy = self.is_busy();

        // 忙碌时禁用输入框
        ui.add_enabled_ui(!busy, |ui| {
            egui::Grid::new("ssh_config")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label("主机:");
                    ui.text_edit_singleline(&mut self.host);
                    ui.end_row();

                    ui.label("端口:");
                    ui.text_edit_singleline(&mut self.port);
                    ui.end_row();

                    ui.label("用户名:");
                    ui.text_edit_singleline(&mut self.username);
                    ui.end_row();

                    ui.label("密码:");
                    let password_edit =
                        egui::TextEdit::singleline(&mut self.password).password(true);
                    ui.add(password_edit);
                    ui.end_row();

                    ui.label("DOMAIN_ID:");
                    ui.text_edit_singleline(&mut self.ros_domain_id);
                    ui.end_row();

                    ui.label("远程目录:");
                    ui.text_edit_singleline(&mut self.remote_dir);
                    ui.end_row();
                });
        });

        ui.add_space(8.0);

        ui.horizontal(|ui| {
            if self.is_connected {
                if ui
                    .add_enabled(!busy, egui::Button::new("断开连接"))
                    .clicked()
                {
                    self.disconnect();
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("刷新列表"))
                    .clicked()
                {
                    self.refresh_file_list();
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("上传文件"))
                    .clicked()
                {
                    self.upload_file();
                }
            } else if ui.add_enabled(!busy, egui::Button::new("连接")).clicked() {
                self.connect(ctx);
            }
        });

        ui.add_space(8.0);

        // 忙碌状态指示
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(self.busy_text());
            });
        }

        // 状态栏
        if !self.status_message.is_empty() {
            ui.colored_label(
                if self.status_message.contains("失败") || self.status_message.contains("错误")
                {
                    egui::Color32::from_rgb(255, 100, 100)
                } else {
                    egui::Color32::from_rgb(100, 255, 100)
                },
                &self.status_message,
            );
            ui.separator();
        }

        // 文件列表
        if !self.file_list.is_empty() {
            ui.heading("文件列表");
            ui.separator();

            let mut clicked_file = None;
            let mut backup_file = None;
            let mut delete_file = None;
            let mut do_upload = false;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let row_width = ui.available_width();
                    let row_height = ui.spacing().interact_size.y;
                    for filename in &self.file_list {
                        let is_selected = self.selected_file.as_ref() == Some(filename);
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(row_width, row_height),
                            egui::Sense::click(),
                        );
                        if ui.is_rect_visible(rect) {
                            let visuals = ui.style().interact_selectable(&resp, is_selected);
                            // 悬停或选中时绘制整行背景
                            if is_selected || resp.hovered() || resp.highlighted() {
                                ui.painter().rect(
                                    rect.expand(visuals.expansion),
                                    visuals.corner_radius,
                                    visuals.weak_bg_fill,
                                    visuals.bg_stroke,
                                    egui::StrokeKind::Inside,
                                );
                            }
                            // 文字左对齐、垂直居中
                            let text_pos = egui::pos2(
                                rect.left() + ui.spacing().button_padding.x,
                                rect.center().y,
                            );
                            ui.painter().text(
                                text_pos,
                                egui::Align2::LEFT_CENTER,
                                filename.as_str(),
                                egui::TextStyle::Body.resolve(ui.style()),
                                visuals.text_color(),
                            );
                        }
                        if resp.clicked() && !busy {
                            clicked_file = Some(filename.clone());
                        }
                        // 右键菜单：上传文件、备份、分割线、删除
                        let fname = filename.clone();
                        let fname2 = filename.clone();
                        resp.context_menu(|ui| {
                            if ui
                                .add_enabled(!busy, egui::Button::new("上传文件"))
                                .clicked()
                            {
                                do_upload = true;
                                ui.close_menu();
                            }
                            if ui.add_enabled(!busy, egui::Button::new("备份")).clicked() {
                                backup_file = Some(fname);
                                ui.close_menu();
                            }
                            ui.separator();
                            let del_btn = egui::Button::new(
                                egui::RichText::new("删除").color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(200, 50, 50));
                            if ui.add_enabled(!busy, del_btn).clicked() {
                                delete_file = Some(fname2);
                                ui.close_menu();
                            }
                        });
                    }

                    // 空白区域占满剩余空间，右键仅「上传文件」
                    let blank_size = ui.available_size().max(egui::vec2(1.0, 1.0));
                    let blank_resp = ui.allocate_response(blank_size, egui::Sense::click());
                    blank_resp.context_menu(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("上传文件"))
                            .clicked()
                        {
                            do_upload = true;
                            ui.close_menu();
                        }
                    });
                });

            if let Some(filename) = clicked_file {
                self.selected_file = Some(filename.clone());
                self.load_file(&filename);
            }
            if let Some(filename) = backup_file {
                self.backup_file(&filename);
            }
            if let Some(filename) = delete_file {
                self.delete_file(&filename);
            }
            if do_upload {
                self.upload_file();
            }
        }
    }

    /// 绘制右侧面板：元数据 + 位姿编辑器 + 操作按钮
    fn right_panel(&mut self, ui: &mut egui::Ui) {
        let Some(data) = &mut self.task_data else {
            ui.centered_and_justified(|ui| {
                ui.label("请选择左侧文件进行编辑");
            });
            return;
        };

        ui.heading("元数据");
        ui.separator();

        egui::Grid::new("metadata")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("map_id:");
                ui.text_edit_singleline(&mut data.map_id);
                ui.end_row();

                ui.label("task_id:");
                ui.text_edit_singleline(&mut data.task_id);
                ui.end_row();
            });

        ui.add_space(12.0);
        ui.heading("位姿编辑");
        ui.separator();

        // 提取 selected_pose_index 避免借用冲突
        let mut selected = self.selected_pose_index;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, field) in data.pose_fields.iter_mut().enumerate() {
                    let is_selected = selected == Some(i);

                    // 选中的位姿用极淡灰色背景 + 细灰边框，不影响阅读
                    let frame = if is_selected {
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgba_premultiplied(30, 30, 30, 25))
                            .stroke(egui::Stroke::new(
                                1.0,
                                egui::Color32::from_rgba_premultiplied(60, 60, 60, 80),
                            ))
                            .inner_margin(6.0)
                            .corner_radius(4.0)
                    } else {
                        egui::Frame::new().inner_margin(6.0)
                    };

                    let available_w = ui.available_width();
                    let frame_resp = frame.show(ui, |ui| {
                        ui.set_min_width(available_w);
                        // 选中时标题用橙色，未选中用默认色
                        let title = if is_selected {
                            egui::RichText::new(&field.key)
                                .strong()
                                .size(14.0)
                                .color(egui::Color32::from_rgb(255, 180, 50))
                        } else {
                            egui::RichText::new(&field.key).strong().size(14.0)
                        };
                        egui::CollapsingHeader::new(title)
                            .default_open(i == 0)
                            .show(ui, |ui| {
                                ui.indent(format!("{}_indent", field.key), |ui| {
                                    Self::draw_single_pose(
                                        ui,
                                        "底盘 (chassis)",
                                        &field.key,
                                        "chassis",
                                        &mut field.pose.chassis_pose,
                                    );
                                    Self::draw_single_pose(
                                        ui,
                                        "头部 (head)",
                                        &field.key,
                                        "head",
                                        &mut field.pose.head_pose,
                                    );
                                    Self::draw_single_pose(
                                        ui,
                                        "腰部 (waist)",
                                        &field.key,
                                        "waist",
                                        &mut field.pose.waist_pose,
                                    );
                                });
                            });
                    });

                    // 检测 frame 区域内的鼠标点击来选中位姿
                    // 不用 interact() 以免吞掉 CollapsingHeader 的点击事件
                    let rect = frame_resp.response.rect;
                    if ui.input(|i| i.pointer.any_click())
                        && ui
                            .input(|i| i.pointer.interact_pos())
                            .is_some_and(|pos| rect.contains(pos))
                    {
                        selected = Some(i);
                    }

                    ui.add_space(4.0);
                }

                ui.add_space(16.0);
            });

        self.selected_pose_index = selected;
    }

    /// 绘制单个 f64 编辑行: `label: [drag_value]`
    fn draw_value_row(ui: &mut egui::Ui, label: &str, val: &mut f64, speed: f64) {
        ui.horizontal(|ui| {
            ui.label(format!("{label}:"));
            ui.add(
                egui::DragValue::new(val)
                    .speed(speed)
                    .custom_formatter(format_f64)
                    .custom_parser(|s| s.parse::<f64>().ok()),
            );
        });
    }

    /// 绘制单个部位的位姿，竖向排列
    ///
    /// ```text
    /// chassis:
    ///   位置:
    ///     x: [value]
    ///     y: [value]
    ///     z: [value]
    ///   姿态:
    ///     w: [value]
    ///     ...
    /// ```
    fn draw_single_pose(
        ui: &mut egui::Ui,
        label: &str,
        _field_key: &str,
        _part: &str,
        pose: &mut Pose,
    ) {
        ui.label(egui::RichText::new(label).underline());
        ui.indent(label, |ui| {
            ui.label("位置:");
            ui.indent(format!("{label}_pos"), |ui| {
                Self::draw_value_row(ui, "x", &mut pose.position.x, 0.01);
                Self::draw_value_row(ui, "y", &mut pose.position.y, 0.01);
                Self::draw_value_row(ui, "z", &mut pose.position.z, 0.01);
            });
            ui.label("姿态:");
            ui.indent(format!("{label}_ori"), |ui| {
                Self::draw_value_row(ui, "w", &mut pose.orientation.w, 0.001);
                Self::draw_value_row(ui, "x", &mut pose.orientation.x, 0.001);
                Self::draw_value_row(ui, "y", &mut pose.orientation.y, 0.001);
                Self::draw_value_row(ui, "z", &mut pose.orientation.z, 0.001);
            });
        });
        ui.add_space(4.0);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 轮询后台线程响应
        self.poll_worker();

        // UI 缩放：Shift + (+/-) 或 Shift + 鼠标滚轮
        // 先用 input_mut 拦截滚轮事件，防止被 ScrollArea 消费掉
        let shift = ctx.input(|i| i.modifiers.shift);
        if shift {
            let mut zoom_delta = 0.0f32;

            // Shift + 键盘 +/-
            if ctx.input(|i| i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
                zoom_delta = 0.1;
            } else if ctx.input(|i| i.key_pressed(egui::Key::Minus)) {
                zoom_delta = -0.1;
            }

            // Shift + 鼠标滚轮（用 input_mut 消费事件，取 x/y 中绝对值较大的）
            ctx.input_mut(|i| {
                for event in &i.raw.events {
                    if let egui::Event::MouseWheel { delta, .. } = event {
                        let scroll = if delta.y.abs() >= delta.x.abs() {
                            delta.y
                        } else {
                            delta.x
                        };
                        if scroll.abs() > 0.01 {
                            zoom_delta += scroll * 0.05;
                        }
                    }
                }
                if zoom_delta.abs() > f32::EPSILON {
                    // 移除滚轮事件，防止同时触发 ScrollArea 滚动
                    i.raw
                        .events
                        .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
                }
            });

            if zoom_delta.abs() > f32::EPSILON {
                self.ui_scale = (self.ui_scale + zoom_delta).clamp(0.5, 3.0);
                ctx.set_pixels_per_point(self.ui_scale);
            }
        }

        let busy = self.is_busy();

        // 左侧面板
        egui::SidePanel::left("left_panel")
            .default_width(280.0)
            .min_width(220.0)
            .show(ctx, |ui| {
                self.left_panel(ctx, ui);
            });

        // 右侧主区域
        egui::CentralPanel::default().show(ctx, |ui| {
            // 顶部操作按钮栏
            ui.horizontal(|ui| {
                if self.task_data.is_some() && self.is_connected {
                    let apply_btn = egui::Button::new(
                        egui::RichText::new("应用到远程文件")
                            .size(14.0)
                            .color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(220, 80, 40));
                    if ui.add_enabled(!busy, apply_btn).clicked() {
                        self.save_to_remote();
                    }

                    ui.add_space(12.0);

                    let has_selection = self.selected_pose_index.is_some();
                    let can_fetch = has_selection && !busy;
                    let no_selection_hint = "请先点击位姿名称选中一个点位";

                    let r1 = ui.add_enabled(
                        can_fetch,
                        egui::Button::new(egui::RichText::new("获取底盘位姿").size(14.0)),
                    );
                    if r1.clicked() {
                        self.fetch_chassis_pose();
                    }
                    if !has_selection {
                        r1.on_hover_text(no_selection_hint);
                    }

                    let r2 = ui.add_enabled(
                        can_fetch,
                        egui::Button::new(egui::RichText::new("获取头部关节").size(14.0)),
                    );
                    if r2.clicked() {
                        self.fetch_head_joints();
                    }
                    if !has_selection {
                        r2.on_hover_text(no_selection_hint);
                    }

                    let r3 = ui.add_enabled(
                        can_fetch,
                        egui::Button::new(egui::RichText::new("获取腰部关节").size(14.0)),
                    );
                    if r3.clicked() {
                        self.fetch_waist_joints();
                    }
                    if !has_selection {
                        r3.on_hover_text(no_selection_hint);
                    }
                }
            });
            ui.separator();

            self.right_panel(ui);
        });
    }
}

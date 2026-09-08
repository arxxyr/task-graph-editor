//! 后台工作线程 — 所有 SSH/SFTP 操作在此线程执行，不阻塞 UI

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ssh::{AuthMethod, DirListing, SshCancellation, SshConfig, SshConnection};

/// UI 唤醒回调：后台线程产生响应后调用，通知 UI 层尽快处理
///
/// 与 GUI 框架解耦——Bevy 侧传入 winit 的 `EventLoopProxy` 唤醒，
/// 测试等无窗口场景可传空闭包。
pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

/// 保存回执只能确认对应文档中对应的一次提交。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveTicket {
    pub document_version: u64,
    pub sequence: u64,
}

/// 加载和删除回执绑定发起时的文档与连接，序号在文档重载后也不复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentReadTicket {
    pub document_version: u64,
    pub connection_generation: u64,
    pub sequence: u64,
}

/// 文件名只能是目录内的单个路径段，不能由 task_id 越界寻址。
pub fn validate_filename(filename: &str) -> Result<(), String> {
    match filename {
        "" | "." | ".." => Err("文件名不能为空或相对目录".into()),
        name if name
            .chars()
            .any(|c| matches!(c, '/' | '\\' | '\0' | '\r' | '\n')) =>
        {
            Err("文件名不能包含路径分隔符或控制字符".into())
        }
        _ => Ok(()),
    }
}

/// UI 忙碌状态
pub enum BusyState {
    /// 空闲
    Idle,
    /// 正在连接
    Connecting,
    /// 正在刷新文件列表
    Refreshing,
    /// 正在加载指定文件
    Loading(String),
    /// 正在执行远程写操作，附带动作描述（如"正在备份 a.json"）
    ///
    /// 保存、备份、删除、上传共用；只说"正在保存"会让备份和删除看着像保存。
    Working(String),
    /// 正在获取指定数据（如"底盘位姿"）
    Fetching(String),
}

/// 发送给后台线程的请求
pub enum WorkerRequest {
    /// 连接 SSH（连接 + 读 ROS_DOMAIN_ID + 列出文件）
    Connect {
        host: String,
        port: u16,
        username: String,
        auth: AuthMethod,
        remote_dir: String,
    },
    /// 断开连接
    Disconnect,
    /// 刷新文件列表
    RefreshFiles { remote_dir: String },
    /// 加载远程文件内容（返回原始 JSON 字符串，解析在 UI 端做）
    LoadFile {
        ticket: DocumentReadTicket,
        remote_dir: String,
        filename: String,
    },
    /// 保存文件（写入 + 可选重命名 + 自动刷新列表）
    SaveFile {
        ticket: SaveTicket,
        remote_dir: String,
        current_filename: String,
        content: String,
        new_filename: Option<String>,
    },
    /// 备份文件（读取 + 生成备份名 + 写入 + 刷新列表）
    BackupFile {
        remote_dir: String,
        filename: String,
        existing_files: Vec<String>,
    },
    /// 删除文件（SFTP unlink + 刷新列表）
    DeleteFile {
        ticket: DocumentReadTicket,
        remote_dir: String,
        filename: String,
    },
    /// 上传本地文件内容到远程（写入 + 刷新列表）
    UploadFile {
        remote_dir: String,
        filename: String,
        content: String,
    },
    /// 执行远程命令
    ExecCommand { command: String },
    /// 执行远程命令（带 stdin 输入）
    ExecCommandWithStdin { command: String, stdin_data: String },
}

/// 后台线程返回的响应
pub enum WorkerResponse {
    /// 连接成功（文件列表可能单独失败，如远程目录写错）
    Connected {
        ros_domain_id: Option<String>,
        file_list: Result<DirListing, String>,
    },
    /// 连接失败
    ConnectFailed(String),
    /// 已断开
    Disconnected,
    /// 文件列表
    FileList(Result<DirListing, String>),
    /// 文件内容加载完成
    FileLoaded {
        ticket: DocumentReadTicket,
        remote_dir: String,
        filename: String,
        result: Result<String, String>,
    },
    /// 保存完成
    FileSaved {
        ticket: SaveTicket,
        remote_dir: String,
        old_filename: String,
        new_filename: Option<String>,
        cleanup_warning: Option<String>,
        file_list: Result<DirListing, String>,
    },
    /// 保存失败
    SaveFailed { ticket: SaveTicket, error: String },
    /// 备份完成
    BackupDone {
        original: String,
        backup_name: String,
        file_list: Result<DirListing, String>,
    },
    /// 备份失败
    BackupFailed(String),
    /// 删除完成
    FileDeleted {
        ticket: DocumentReadTicket,
        remote_dir: String,
        filename: String,
        file_list: Result<DirListing, String>,
    },
    /// 删除失败
    DeleteFailed {
        ticket: DocumentReadTicket,
        error: String,
    },
    /// 上传完成
    FileUploaded {
        filename: String,
        file_list: Result<DirListing, String>,
    },
    /// 上传失败
    UploadFailed(String),
    /// 命令执行结果
    CommandOutput(Result<String, String>),
    /// SSH 连接丢失（keepalive 失败）
    ConnectionLost(String),
    /// 正在自动重连（attempt=0 表示尚未尝试，delay_secs 为下次重试等待秒数）
    Reconnecting { attempt: u32, delay_secs: u64 },
    /// 自动重连成功（文件列表可能单独失败）
    Reconnected {
        ros_domain_id: Option<String>,
        file_list: Result<DirListing, String>,
    },
}

/// App 持有的句柄，用于与后台线程通信
///
/// `Receiver` 本身不是 `Sync`，而 Bevy 的 `Resource` 要求 `Send + Sync`，
/// 故用 `Mutex` 包一层；接收只发生在 UI 线程，锁没有竞争。
pub struct WorkerHandle {
    tx: mpsc::Sender<WorkerRequest>,
    rx: Mutex<mpsc::Receiver<WorkerResponse>>,
    cancellation: SshCancellation,
}

impl WorkerHandle {
    /// 启动后台工作线程，返回通信句柄
    pub fn spawn(wake: WakeFn) -> std::io::Result<Self> {
        let (req_tx, req_rx) = mpsc::channel::<WorkerRequest>();
        let (resp_tx, resp_rx) = mpsc::channel::<WorkerResponse>();

        let cancellation = SshCancellation::default();
        let worker_cancellation = cancellation.clone();
        std::thread::Builder::new()
            .name("ssh-worker".into())
            .spawn(move || worker_loop(req_rx, resp_tx, wake, worker_cancellation))?;

        Ok(Self {
            tx: req_tx,
            rx: Mutex::new(resp_rx),
            cancellation,
        })
    }

    /// 发送请求到后台线程
    pub fn send(&self, request: WorkerRequest) {
        if matches!(request, WorkerRequest::Disconnect) {
            self.cancellation.cancel();
        }
        // 发送失败说明后台线程已退出，忽略即可
        let _ = self.tx.send(request);
    }

    /// 非阻塞接收一个响应（UI 轮询时调用）
    pub fn try_recv(&self) -> Option<WorkerResponse> {
        self.rx.lock().ok()?.try_recv().ok()
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        // 不能等待工作线程自己轮询 Disconnect，套接字上的阻塞操作也必须被打断。
        self.cancellation.cancel();
        let _ = self.tx.send(WorkerRequest::Disconnect);
    }
}

/// Keepalive 轮询间隔：每 15 秒检查一次是否需要发送心跳
///
/// 略短于 SSH keepalive 间隔（30s），确保在间隔窗口内至少调用一次 `keepalive_send()`。
const KEEPALIVE_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// 指数退避初始延迟
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_secs(2);

/// 指数退避最大延迟
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(60);

/// 自动重连状态（指数退避）
struct ReconnectState {
    /// 已尝试重连次数
    attempt: u32,
    /// 当前退避延迟
    delay: Duration,
    /// 下次尝试重连的时间点
    next_try: Instant,
}

impl ReconnectState {
    fn new() -> Self {
        Self {
            attempt: 0,
            delay: RECONNECT_INITIAL_DELAY,
            next_try: Instant::now() + RECONNECT_INITIAL_DELAY,
        }
    }

    /// 重连失败后增加退避延迟（翻倍，不超过最大值）
    fn backoff(&mut self) {
        self.delay = (self.delay * 2).min(RECONNECT_MAX_DELAY);
        self.next_try = Instant::now() + self.delay;
    }
}

/// 列出远程 JSON 文件，失败时记日志并返回空表
///
/// 连接流程不因列不出文件而中止（连接本身是成功的），但错误必须留痕——
/// 早先这里是 `unwrap_or_default()`，目录写错时界面只显示"已连接"、
/// 列表空空如也且没有任何提示。
fn list_files_logged(conn: &SshConnection, remote_dir: &str) -> Result<DirListing, String> {
    match conn.list_json_files(remote_dir) {
        Ok(listing) => {
            tracing::info!(
                dir = %listing.resolved_dir,
                json = listing.json_files.len(),
                entries = listing.total_entries,
                "已获取文件列表"
            );
            Ok(listing)
        }
        Err(e) => {
            tracing::warn!(dir = %remote_dir, error = %e, "获取文件列表失败");
            Err(e.to_string())
        }
    }
}

/// 尝试读取远程 ROS_DOMAIN_ID
fn read_ros_domain_id(conn: &SshConnection) -> Option<String> {
    conn.exec_command("bash -ic 'echo $ROS_DOMAIN_ID' 2>/dev/null")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 后台工作线程主循环
///
/// 三种运行模式：
/// 1. **已连接**：处理请求，空闲时发送 keepalive 心跳
/// 2. **重连中**：指数退避自动重连，期间仍响应 Disconnect 等请求
/// 3. **未连接**：阻塞等待 Connect 请求
fn worker_loop(
    rx: mpsc::Receiver<WorkerRequest>,
    tx: mpsc::Sender<WorkerResponse>,
    wake: WakeFn,
    cancellation: SshCancellation,
) {
    let mut connection: Option<SshConnection> = None;
    // 保存连接参数，用于自动重连时重建连接
    let mut connect_params: Option<(SshConfig, String)> = None;
    let mut reconnect: Option<ReconnectState> = None;

    /// 发送响应并唤醒 UI
    macro_rules! respond {
        ($resp:expr) => {{
            let _ = tx.send($resp);
            (wake)();
        }};
    }

    loop {
        if cancellation.is_cancelled() {
            break;
        }
        // ── 重连：到时间就尝试 ──
        if let Some(ref mut rs) = reconnect
            && Instant::now() >= rs.next_try
        {
            rs.attempt += 1;
            if let Some((config, remote_dir)) = &connect_params {
                match SshConnection::connect_cancellable(config, &cancellation) {
                    Ok(conn) => {
                        let ros_domain_id = read_ros_domain_id(&conn);
                        let file_list = list_files_logged(&conn, remote_dir);
                        connection = Some(conn);
                        reconnect = None;
                        respond!(WorkerResponse::Reconnected {
                            ros_domain_id,
                            file_list,
                        });
                    }
                    Err(_) => {
                        rs.backoff();
                        respond!(WorkerResponse::Reconnecting {
                            attempt: rs.attempt,
                            delay_secs: rs.delay.as_secs(),
                        });
                    }
                }
            } else {
                reconnect = None;
            }
            continue;
        }

        // ── 接收请求 ──
        let request = if let Some(ref rs) = reconnect {
            // 重连中：短轮询（最多 1 秒），保证及时响应 Disconnect
            let wait = rs
                .next_try
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(1));
            match rx.recv_timeout(wait) {
                Ok(req) => req,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else if connection.is_some() {
            // 已连接：keepalive 轮询
            match rx.recv_timeout(KEEPALIVE_POLL_INTERVAL) {
                Ok(req) => req,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(conn) = &connection
                        && let Err(e) = conn.send_keepalive()
                    {
                        connection = None;
                        let rs = ReconnectState::new();
                        respond!(WorkerResponse::ConnectionLost(format!("SSH 心跳失败: {e}")));
                        respond!(WorkerResponse::Reconnecting {
                            attempt: 0,
                            delay_secs: rs.delay.as_secs(),
                        });
                        reconnect = Some(rs);
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            // 未连接：阻塞等待
            match rx.recv() {
                Ok(req) => req,
                Err(_) => break,
            }
        };

        if cancellation.is_cancelled() {
            break;
        }
        // ── 处理请求 ──
        match request {
            WorkerRequest::Connect {
                host,
                port,
                username,
                auth,
                remote_dir,
            } => {
                reconnect = None; // 取消进行中的自动重连
                connection = None;
                connect_params = None;
                let config = SshConfig {
                    host,
                    port,
                    username,
                    auth,
                };

                tracing::info!(host = %config.host, port, user = %config.username, remote_dir = %remote_dir, "开始连接");
                match SshConnection::connect_cancellable(&config, &cancellation) {
                    Ok(conn) => {
                        let ros_domain_id = read_ros_domain_id(&conn);
                        let file_list = list_files_logged(&conn, &remote_dir);
                        let remote_dir = file_list
                            .as_ref()
                            .map_or(remote_dir, |listing| listing.resolved_dir.clone());
                        connect_params = Some((config, remote_dir));
                        connection = Some(conn);
                        respond!(WorkerResponse::Connected {
                            ros_domain_id,
                            file_list,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "SSH 连接失败");
                        respond!(WorkerResponse::ConnectFailed(e.to_string()));
                    }
                }
            }

            WorkerRequest::Disconnect => {
                connection = None;
                reconnect = None;
                connect_params = None;
                respond!(WorkerResponse::Disconnected);
            }

            WorkerRequest::RefreshFiles { remote_dir } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::FileList(Err("未连接".into())));
                    continue;
                };
                let result = list_files_logged(conn, &remote_dir);
                if let Ok(listing) = &result
                    && let Some((_, directory)) = &mut connect_params
                {
                    directory.clone_from(&listing.resolved_dir);
                }
                respond!(WorkerResponse::FileList(result));
            }

            WorkerRequest::LoadFile {
                ticket,
                remote_dir,
                filename,
            } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::FileLoaded {
                        ticket,
                        remote_dir,
                        filename,
                        result: Err("未连接".into()),
                    });
                    continue;
                };
                let remote_dir = match conn.canonical_dir(&remote_dir) {
                    Ok(directory) => directory,
                    Err(error) => {
                        respond!(WorkerResponse::FileLoaded {
                            ticket,
                            remote_dir,
                            filename,
                            result: Err(error.to_string()),
                        });
                        continue;
                    }
                };
                let path = format!("{remote_dir}/{filename}");
                let result = conn.read_file(&path).map_err(|e| e.to_string());
                respond!(WorkerResponse::FileLoaded {
                    ticket,
                    remote_dir,
                    filename,
                    result
                });
            }

            WorkerRequest::SaveFile {
                ticket,
                remote_dir,
                current_filename,
                content,
                new_filename,
            } => {
                let valid_names = validate_filename(&current_filename)
                    .and_then(|()| new_filename.as_deref().map_or(Ok(()), validate_filename));
                if let Err(error) = valid_names {
                    respond!(WorkerResponse::SaveFailed { ticket, error });
                    continue;
                }
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::SaveFailed {
                        ticket,
                        error: "未连接".into()
                    });
                    continue;
                };

                let current_path = format!("{remote_dir}/{current_filename}");

                let new_path = new_filename
                    .as_ref()
                    .map(|name| format!("{remote_dir}/{name}"));
                let outcome = match conn.save_file(&current_path, &content, new_path.as_deref()) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        respond!(WorkerResponse::SaveFailed {
                            ticket,
                            error: error.to_string()
                        });
                        continue;
                    }
                };
                tracing::info!(path = %outcome.saved_path, "任务图已提交");

                // 刷新文件列表
                let file_list = list_files_logged(conn, &remote_dir);
                respond!(WorkerResponse::FileSaved {
                    ticket,
                    remote_dir,
                    old_filename: current_filename,
                    new_filename,
                    cleanup_warning: outcome.cleanup_warning,
                    file_list,
                });
            }

            WorkerRequest::BackupFile {
                remote_dir,
                filename,
                existing_files,
            } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::BackupFailed("未连接".into()));
                    continue;
                };

                let source_path = format!("{remote_dir}/{filename}");

                // 读取源文件内容
                let content = match conn.read_file(&source_path) {
                    Ok(c) => c,
                    Err(e) => {
                        respond!(WorkerResponse::BackupFailed(format!("读取源文件失败: {e}")));
                        continue;
                    }
                };

                // 生成备份文件名
                let stem = filename.strip_suffix(".json").unwrap_or(&filename);
                let timestamp = chrono::Local::now().format("%Y%m%d%H%M").to_string();
                let mut backup_name = format!("{stem}-backup-{timestamp}.json");

                // 检查文件名冲突，添加数字后缀
                if existing_files.contains(&backup_name) {
                    let mut counter = 1u32;
                    loop {
                        backup_name = format!("{stem}-backup-{timestamp}-{counter}.json");
                        if !existing_files.contains(&backup_name) {
                            break;
                        }
                        counter += 1;
                    }
                }

                let backup_path = format!("{remote_dir}/{backup_name}");

                // 写入备份文件
                if let Err(e) = conn.write_new_file(&backup_path, &content) {
                    respond!(WorkerResponse::BackupFailed(format!(
                        "写入备份文件失败: {e}"
                    )));
                    continue;
                }

                // 刷新文件列表
                let file_list = list_files_logged(conn, &remote_dir);
                respond!(WorkerResponse::BackupDone {
                    original: filename,
                    backup_name,
                    file_list,
                });
            }

            WorkerRequest::DeleteFile {
                ticket,
                remote_dir,
                filename,
            } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::DeleteFailed {
                        ticket,
                        error: "未连接".into()
                    });
                    continue;
                };

                let path = format!("{remote_dir}/{filename}");
                if let Err(e) = conn.delete_file(&path) {
                    respond!(WorkerResponse::DeleteFailed {
                        ticket,
                        error: e.to_string()
                    });
                    continue;
                }

                // 刷新文件列表
                let file_list = list_files_logged(conn, &remote_dir);
                respond!(WorkerResponse::FileDeleted {
                    ticket,
                    remote_dir,
                    filename,
                    file_list,
                });
            }

            WorkerRequest::UploadFile {
                remote_dir,
                filename,
                content,
            } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::UploadFailed("未连接".into()));
                    continue;
                };

                let path = format!("{remote_dir}/{filename}");
                if let Err(e) = conn.write_new_file(&path, &content) {
                    respond!(WorkerResponse::UploadFailed(e.to_string()));
                    continue;
                }

                // 刷新文件列表
                let file_list = list_files_logged(conn, &remote_dir);
                respond!(WorkerResponse::FileUploaded {
                    filename,
                    file_list,
                });
            }

            WorkerRequest::ExecCommand { command } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::CommandOutput(Err("未连接".into())));
                    continue;
                };
                let result = conn.exec_command(&command).map_err(|e| e.to_string());
                respond!(WorkerResponse::CommandOutput(result));
            }

            WorkerRequest::ExecCommandWithStdin {
                command,
                stdin_data,
            } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::CommandOutput(Err("未连接".into())));
                    continue;
                };
                let result = conn
                    .exec_command_with_stdin(&command, &stdin_data)
                    .map_err(|e| e.to_string());
                respond!(WorkerResponse::CommandOutput(result));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> (WorkerHandle, SshCancellation) {
        let (tx, _requests) = mpsc::channel();
        let (_responses, rx) = mpsc::channel();
        let cancellation = SshCancellation::default();
        (
            WorkerHandle {
                tx,
                rx: Mutex::new(rx),
                cancellation: cancellation.clone(),
            },
            cancellation,
        )
    }

    #[test]
    fn 断开和销毁句柄不依赖后台读取消息就取消() {
        let (worker, cancellation) = handle();
        worker.send(WorkerRequest::Disconnect);
        assert!(cancellation.is_cancelled());
        let (worker, cancellation) = handle();
        drop(worker);
        assert!(cancellation.is_cancelled());
    }

    #[test]
    fn 文件名不能越出绑定目录但支持中文和引号() {
        for name in [
            "../task.json",
            "sub/task.json",
            "sub\\task.json",
            "\0.json",
            "\n.json",
            "",
            ".",
            "..",
        ] {
            assert!(validate_filename(name).is_err(), "应拒绝 {name:?}");
        }
        for name in ["task.json", "任务 '$ 文件.json"] {
            assert!(validate_filename(name).is_ok());
        }
    }
}

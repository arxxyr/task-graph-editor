//! 后台工作线程 — 所有 SSH/SFTP 操作在此线程执行，不阻塞 UI

use std::sync::mpsc;

use eframe::egui;

use crate::ssh::{AuthMethod, SshConfig, SshConnection};

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
    /// 正在保存
    Saving,
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
        password: String,
        remote_dir: String,
    },
    /// 断开连接
    Disconnect,
    /// 刷新文件列表
    RefreshFiles { remote_dir: String },
    /// 加载远程文件内容（返回原始 JSON 字符串，解析在 UI 端做）
    LoadFile {
        remote_dir: String,
        filename: String,
    },
    /// 保存文件（写入 + 可选重命名 + 自动刷新列表）
    SaveFile {
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
    /// 删除文件（rm + 刷新列表）
    DeleteFile {
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
    /// 连接成功
    Connected {
        ros_domain_id: Option<String>,
        file_list: Vec<String>,
    },
    /// 连接失败
    ConnectFailed(String),
    /// 已断开
    Disconnected,
    /// 文件列表
    FileList(Result<Vec<String>, String>),
    /// 文件内容加载完成
    FileLoaded {
        filename: String,
        result: Result<String, String>,
    },
    /// 保存完成
    FileSaved {
        old_filename: String,
        new_filename: Option<String>,
        file_list: Result<Vec<String>, String>,
    },
    /// 保存失败
    SaveFailed(String),
    /// 备份完成
    BackupDone {
        original: String,
        backup_name: String,
        file_list: Result<Vec<String>, String>,
    },
    /// 备份失败
    BackupFailed(String),
    /// 删除完成
    FileDeleted {
        filename: String,
        file_list: Result<Vec<String>, String>,
    },
    /// 删除失败
    DeleteFailed(String),
    /// 上传完成
    FileUploaded {
        filename: String,
        file_list: Result<Vec<String>, String>,
    },
    /// 上传失败
    UploadFailed(String),
    /// 命令执行结果
    CommandOutput(Result<String, String>),
}

/// App 持有的句柄，用于与后台线程通信
pub struct WorkerHandle {
    tx: mpsc::Sender<WorkerRequest>,
    rx: mpsc::Receiver<WorkerResponse>,
}

impl WorkerHandle {
    /// 启动后台工作线程，返回通信句柄
    pub fn spawn(ctx: egui::Context) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<WorkerRequest>();
        let (resp_tx, resp_rx) = mpsc::channel::<WorkerResponse>();

        std::thread::Builder::new()
            .name("ssh-worker".into())
            .spawn(move || worker_loop(req_rx, resp_tx, ctx))
            .expect("启动后台工作线程失败");

        Self {
            tx: req_tx,
            rx: resp_rx,
        }
    }

    /// 发送请求到后台线程
    pub fn send(&self, request: WorkerRequest) {
        // 发送失败说明后台线程已退出，忽略即可
        let _ = self.tx.send(request);
    }

    /// 非阻塞接收一个响应（update() 中调用）
    pub fn try_recv(&self) -> Option<WorkerResponse> {
        self.rx.try_recv().ok()
    }
}

/// 后台工作线程主循环
fn worker_loop(
    rx: mpsc::Receiver<WorkerRequest>,
    tx: mpsc::Sender<WorkerResponse>,
    ctx: egui::Context,
) {
    let mut connection: Option<SshConnection> = None;

    /// 发送响应并请求 UI 重绘
    macro_rules! respond {
        ($resp:expr) => {
            let _ = tx.send($resp);
            ctx.request_repaint();
        };
    }

    while let Ok(request) = rx.recv() {
        match request {
            WorkerRequest::Connect {
                host,
                port,
                username,
                password,
                remote_dir,
            } => {
                let config = SshConfig {
                    host,
                    port,
                    username,
                    auth: AuthMethod::Password(password),
                };

                match SshConnection::connect(&config) {
                    Ok(conn) => {
                        // 尝试读取 ROS_DOMAIN_ID
                        let ros_domain_id = conn
                            .exec_command("bash -ic 'echo $ROS_DOMAIN_ID' 2>/dev/null")
                            .ok()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());

                        // 列出远程文件
                        let file_list = conn.list_json_files(&remote_dir).unwrap_or_default();

                        connection = Some(conn);
                        respond!(WorkerResponse::Connected {
                            ros_domain_id,
                            file_list,
                        });
                    }
                    Err(e) => {
                        respond!(WorkerResponse::ConnectFailed(e.to_string()));
                    }
                }
            }

            WorkerRequest::Disconnect => {
                connection = None;
                respond!(WorkerResponse::Disconnected);
            }

            WorkerRequest::RefreshFiles { remote_dir } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::FileList(Err("未连接".into())));
                    continue;
                };
                let result = conn.list_json_files(&remote_dir).map_err(|e| e.to_string());
                respond!(WorkerResponse::FileList(result));
            }

            WorkerRequest::LoadFile {
                remote_dir,
                filename,
            } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::FileLoaded {
                        filename,
                        result: Err("未连接".into()),
                    });
                    continue;
                };
                let path = format!("{remote_dir}/{filename}");
                let result = conn.read_file(&path).map_err(|e| e.to_string());
                respond!(WorkerResponse::FileLoaded { filename, result });
            }

            WorkerRequest::SaveFile {
                remote_dir,
                current_filename,
                content,
                new_filename,
            } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::SaveFailed("未连接".into()));
                    continue;
                };

                let current_path = format!("{remote_dir}/{current_filename}");

                // 写入文件
                if let Err(e) = conn.write_file(&current_path, &content) {
                    respond!(WorkerResponse::SaveFailed(e.to_string()));
                    continue;
                }

                // 可选重命名
                if let Some(ref new_name) = new_filename {
                    let new_path = format!("{remote_dir}/{new_name}");
                    if let Err(e) = conn.rename_file(&current_path, &new_path) {
                        respond!(WorkerResponse::SaveFailed(format!(
                            "文件已保存但重命名失败: {e}"
                        )));
                        continue;
                    }
                }

                // 刷新文件列表
                let file_list = conn.list_json_files(&remote_dir).map_err(|e| e.to_string());
                respond!(WorkerResponse::FileSaved {
                    old_filename: current_filename,
                    new_filename,
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
                if let Err(e) = conn.write_file(&backup_path, &content) {
                    respond!(WorkerResponse::BackupFailed(format!(
                        "写入备份文件失败: {e}"
                    )));
                    continue;
                }

                // 刷新文件列表
                let file_list = conn.list_json_files(&remote_dir).map_err(|e| e.to_string());
                respond!(WorkerResponse::BackupDone {
                    original: filename,
                    backup_name,
                    file_list,
                });
            }

            WorkerRequest::DeleteFile {
                remote_dir,
                filename,
            } => {
                let Some(conn) = connection.as_ref() else {
                    respond!(WorkerResponse::DeleteFailed("未连接".into()));
                    continue;
                };

                let path = format!("{remote_dir}/{filename}");
                if let Err(e) = conn.exec_command(&format!("rm -f '{path}'")) {
                    respond!(WorkerResponse::DeleteFailed(e.to_string()));
                    continue;
                }

                // 刷新文件列表
                let file_list = conn.list_json_files(&remote_dir).map_err(|e| e.to_string());
                respond!(WorkerResponse::FileDeleted {
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
                if let Err(e) = conn.write_file(&path, &content) {
                    respond!(WorkerResponse::UploadFailed(e.to_string()));
                    continue;
                }

                // 刷新文件列表
                let file_list = conn.list_json_files(&remote_dir).map_err(|e| e.to_string());
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

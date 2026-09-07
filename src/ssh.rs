//! SSH/SFTP 远程文件操作模块

use ssh2::Session;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

mod agent;
mod atomic_write;
mod cancellation;
pub use cancellation::SshCancellation;

/// SSH 认证方式
pub enum AuthMethod {
    /// 密码认证
    Password(String),
    /// 公钥认证：先尝试 ssh-agent 中的全部身份，再按顺序尝试给定私钥文件（不带口令）
    PublicKey { identity_files: Vec<PathBuf> },
}

/// SSH 连接配置
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
}

/// SSH 操作错误
#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("网络或文件 I/O 失败: {0}")]
    TcpConnect(#[from] std::io::Error),
    #[error("SSH 操作失败: {0}")]
    Ssh(#[from] ssh2::Error),
    #[error("文件读取编码错误: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("命令执行失败 (exit={exit_code}): {output}")]
    CommandFailed { exit_code: i32, output: String },
    #[error("认证失败: {0}")]
    AuthFailed(String),
    #[error("连接已取消")]
    Cancelled,
    #[error("连接状态异常: {0}")]
    ConnectionState(String),
    #[error("原子保存 {path} 失败: {reason}")]
    AtomicWrite { path: String, reason: String },
}

/// SSH keepalive 间隔（秒）
///
/// 每隔此间隔发送一次 SSH keepalive 包，防止空闲连接被服务器或中间网络设备（NAT/防火墙）超时断开。
const KEEPALIVE_INTERVAL_SECS: u32 = 30;

/// 会话自身必须设置 API 超时，TCP 超时不能限制 libssh2 内部的 poll。
fn handshake(tcp: TcpStream, timeout_ms: u32) -> Result<Session, SshError> {
    let timeout = std::time::Duration::from_millis(u64::from(timeout_ms));
    tcp.set_read_timeout(Some(timeout))?;
    tcp.set_write_timeout(Some(timeout))?;
    let mut session = Session::new()?;
    session.set_timeout(timeout_ms);
    session.set_tcp_stream(tcp);
    session.handshake()?;
    Ok(session)
}

/// 公钥认证：ssh-agent 全部身份 → 私钥文件逐个尝试；全部失败时汇总每一步的原因
fn authenticate_public_key(
    session: &Session,
    username: &str,
    identity_files: &[PathBuf],
    cancellation: &SshCancellation,
) -> Result<(), SshError> {
    let mut failures: Vec<String> = Vec::new();

    match agent::authenticate(session, username, cancellation) {
        Ok(()) => return Ok(()),
        Err(reason) => failures.push(reason),
    }

    cancellation.check()?;
    for key in identity_files {
        cancellation.check()?;
        match session.userauth_pubkey_file(username, None, key, None) {
            Ok(()) => return Ok(()),
            Err(e) => failures.push(format!("私钥 {}: {e}", key.display())),
        }
    }
    if identity_files.is_empty() {
        failures.push("未找到可用的私钥文件（~/.ssh/id_rsa / id_ecdsa / id_ed25519）".into());
    }
    failures.push("密码为空，未尝试密码认证".into());

    Err(SshError::AuthFailed(failures.join("；")))
}

/// 展开路径开头的 `~`，并规整结尾斜杠
///
/// SFTP 协议不做 shell 展开：`~/Workspace/task_graphs` 会被原样当成路径，
/// 服务端按相对路径解析成 `<home>/~/Workspace/task_graphs`，直接报"没有那个文件或目录"。
/// 而在远程目录里填 `~/...` 是很自然的写法，所以这里自己替换成绝对路径。
///
/// `home` 取不到时保持原样——总比猜一个错的路径强。
/// 只认 `~` 与 `~/`；`~user/...` 这种要查 passwd 才能解析，不处理。
fn expand_home(path: &str, home: Option<&str>) -> String {
    let path = path.trim();
    let expanded = match (home, path.strip_prefix("~/")) {
        (Some(home), Some(rest)) => format!("{}/{rest}", home.trim_end_matches('/')),
        (Some(home), None) if path == "~" => home.trim_end_matches('/').to_string(),
        _ => path.to_string(),
    };
    // 结尾斜杠对 readdir 无害，但拼子路径时会拼出 `//`；根目录要留着
    match expanded.as_str() {
        "/" => expanded,
        _ => expanded.trim_end_matches('/').to_string(),
    }
}

/// 一次目录扫描的结果
///
/// 只返回文件名列表的话，"没找到文件"就分不清是路径写错、目录为空、
/// 还是 JSON 在子目录里。把这些线索一并带回来，界面才能给出可操作的提示。
#[derive(Debug, Clone, Default)]
pub struct DirListing {
    /// 展开 `~` 之后实际扫描的路径
    pub resolved_dir: String,
    /// 目录下的 `.json` 文件名（已排序）
    pub json_files: Vec<String>,
    /// 目录下的条目总数，含非 JSON 文件与子目录
    pub total_entries: usize,
    /// 子目录名（已排序），用于提示"是不是要找的在下一层"
    pub subdirs: Vec<String>,
}

/// 封装 SSH 会话，提供文件操作接口
pub struct SshConnection {
    session: Session,
    /// 远程 home 目录，用于展开路径里的 `~`
    ///
    /// SFTP 协议不做 shell 展开，`~/x` 会被当成名字就叫 `~` 的目录，
    /// 必须自己替换成绝对路径。取不到时保持原样。
    home: Option<String>,
}

impl SshConnection {
    /// 建立 SSH 连接并认证
    pub fn connect_cancellable(
        config: &SshConfig,
        cancellation: &SshCancellation,
    ) -> Result<Self, SshError> {
        let tcp = cancellation::connect_tcp(config, cancellation)?;
        let session = handshake(tcp, 30_000)?;
        cancellation.check()?;

        match &config.auth {
            AuthMethod::Password(password) => {
                session.userauth_password(&config.username, password)?;
            }
            AuthMethod::PublicKey { identity_files } => {
                authenticate_public_key(&session, &config.username, identity_files, cancellation)?;
            }
        }

        // 启用 SSH keepalive：定期发送心跳包，want_reply=true 以便检测对端是否存活
        session.set_keepalive(true, KEEPALIVE_INTERVAL_SECS);

        let mut connection = Self {
            session,
            home: None,
        };
        connection.home = connection.query_home();
        tracing::info!(
            host = %config.host,
            port = config.port,
            user = %config.username,
            home = ?connection.home,
            "SSH 已连接并认证"
        );
        Ok(connection)
    }

    /// 询问远程 home 目录（失败不致命，只是 `~` 展开会失效）
    fn query_home(&self) -> Option<String> {
        let home = self.exec_command("printf %s \"$HOME\"").ok()?;
        let home = home.trim();
        match home.is_empty() {
            true => None,
            false => Some(home.to_string()),
        }
    }

    /// 展开路径开头的 `~`，并去掉多余的结尾斜杠
    pub fn resolve_path(&self, path: &str) -> String {
        expand_home(path, self.home.as_deref())
    }

    /// 发送 SSH keepalive 心跳包
    ///
    /// 返回下次应发送心跳的秒数。连接已断开时返回 Err。
    pub fn send_keepalive(&self) -> Result<u32, SshError> {
        Ok(self.session.keepalive_send()?)
    }

    /// 由 SFTP 服务端展开为绝对目录，绑定文档时不能留下相对路径。
    pub fn canonical_dir(&self, dir: &str) -> Result<String, SshError> {
        let resolved = self.resolve_path(dir);
        let sftp = self.session.sftp()?;
        let absolute = sftp.realpath(Path::new(&resolved))?;
        let absolute = absolute
            .to_str()
            .ok_or_else(|| SshError::ConnectionState("远程目录不是有效 UTF-8".into()))?;
        if !absolute.starts_with('/') {
            return Err(SshError::ConnectionState("SFTP 未返回绝对目录".into()));
        }
        Ok(absolute.to_owned())
    }

    /// 扫描远程目录，列出 `.json` 文件并附带排查线索
    pub fn list_json_files(&self, dir: &str) -> Result<DirListing, SshError> {
        let resolved = self.canonical_dir(dir)?;
        tracing::debug!(input = dir, resolved = %resolved, "SFTP readdir");
        let sftp = self.session.sftp()?;
        let entries = sftp.readdir(Path::new(&resolved)).map_err(|e| {
            tracing::warn!(dir = %resolved, error = %e, "列出远程目录失败");
            e
        })?;

        let total_entries = entries.len();
        let mut json_files = Vec::new();
        let mut subdirs = Vec::new();
        for (path, stat) in entries {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            match (stat.is_dir(), name.ends_with(".json")) {
                (true, _) => subdirs.push(name),
                (false, true) => json_files.push(name),
                (false, false) => {}
            }
        }
        json_files.sort();
        subdirs.sort();

        tracing::debug!(
            dir = %resolved,
            json = json_files.len(),
            entries = total_entries,
            subdirs = subdirs.len(),
            "远程目录扫描完成"
        );
        Ok(DirListing {
            resolved_dir: resolved,
            json_files,
            total_entries,
            subdirs,
        })
    }

    /// 读取远程文件内容
    pub fn read_file(&self, path: &str) -> Result<String, SshError> {
        let resolved = self.resolve_path(path);
        tracing::debug!(path = %resolved, "SFTP 读取文件");
        let sftp = self.session.sftp()?;
        let mut file = sftp.open(Path::new(&resolved))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(String::from_utf8(buf)?)
    }

    /// 删除远程文件
    ///
    /// 走 SFTP 而非 `rm`：原先的 `rm -f '<path>'` 把路径放进单引号里，
    /// shell 不会展开其中的 `~`，删除会静默失败。
    pub fn delete_file(&self, path: &str) -> Result<(), SshError> {
        let resolved = self.resolve_path(path);
        tracing::debug!(path = %resolved, "SFTP 删除文件");
        let sftp = self.session.sftp()?;
        sftp.unlink(Path::new(&resolved))?;
        Ok(())
    }

    /// 执行远程命令并返回 stdout
    pub fn exec_command(&self, command: &str) -> Result<String, SshError> {
        let mut channel = self.session.channel_session()?;
        channel.handle_extended_data(ssh2::ExtendedData::Merge)?;
        channel.exec(command)?;

        let mut output = String::new();
        channel.read_to_string(&mut output)?;
        channel.wait_close()?;

        let exit_status = channel.exit_status()?;
        if exit_status != 0 {
            return Err(SshError::CommandFailed {
                exit_code: exit_status,
                output,
            });
        }

        Ok(output)
    }

    /// 执行远程命令，通过 stdin 传入脚本内容，返回 stdout
    ///
    /// 适用于 `python3 -` 等从标准输入读取脚本的场景。
    pub fn exec_command_with_stdin(
        &self,
        command: &str,
        stdin_data: &str,
    ) -> Result<String, SshError> {
        let mut channel = self.session.channel_session()?;
        channel.handle_extended_data(ssh2::ExtendedData::Merge)?;
        channel.exec(command)?;

        // 写入 stdin 后关闭写端，让远程进程收到 EOF
        channel.write_all(stdin_data.as_bytes())?;
        channel.send_eof()?;

        let mut output = String::new();
        channel.read_to_string(&mut output)?;
        channel.wait_close()?;

        let exit_status = channel.exit_status()?;
        if exit_status != 0 {
            return Err(SshError::CommandFailed {
                exit_code: exit_status,
                output,
            });
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::{expand_home, handshake};

    #[test]
    fn 不发送banner的本地服务受会话api时限约束() {
        use std::net::{TcpListener, TcpStream};
        use std::time::{Duration, Instant};
        // 仅 loopback，无远端与睡眠；服务端持有 socket 但永不发送 banner。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let started = Instant::now();
        assert!(handshake(socket, 100).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn 展开波浪线开头的路径() {
        assert_eq!(
            expand_home("~/Workspace/task_graphs", Some("/home/linux")),
            "/home/linux/Workspace/task_graphs"
        );
    }

    #[test]
    fn 展开时去掉结尾斜杠() {
        // 用户在界面里习惯性带上结尾斜杠，拼子路径时会变成 //
        assert_eq!(
            expand_home("~/Workspace/task_graphs/", Some("/home/linux")),
            "/home/linux/Workspace/task_graphs"
        );
    }

    #[test]
    fn 单独的波浪线展开为home() {
        assert_eq!(expand_home("~", Some("/home/linux")), "/home/linux");
    }

    #[test]
    fn home自带结尾斜杠不产生双斜杠() {
        assert_eq!(expand_home("~/x", Some("/home/linux/")), "/home/linux/x");
    }

    #[test]
    fn 取不到home时保持原样() {
        // 猜一个错的路径不如原样传给服务端，让错误信息如实反映用户输入
        assert_eq!(expand_home("~/x", None), "~/x");
    }

    #[test]
    fn 绝对路径只规整结尾斜杠() {
        assert_eq!(
            expand_home("/home/linux/task_graphs/", Some("/home/linux")),
            "/home/linux/task_graphs"
        );
    }

    #[test]
    fn 相对路径不动() {
        assert_eq!(
            expand_home("Workspace/tg", Some("/home/linux")),
            "Workspace/tg"
        );
    }

    #[test]
    fn 根目录保留斜杠() {
        assert_eq!(expand_home("/", Some("/home/linux")), "/");
    }

    #[test]
    fn 前后空格被去掉() {
        assert_eq!(expand_home("  ~/x  ", Some("/home/linux")), "/home/linux/x");
    }

    #[test]
    fn 波浪线用户名形式不展开() {
        // ~user 要查 passwd 才能解析，不处理，原样传给服务端
        assert_eq!(expand_home("~root/x", Some("/home/linux")), "~root/x");
    }

    #[test]
    fn 多个结尾斜杠都去掉() {
        assert_eq!(expand_home("~/x///", Some("/home/linux")), "/home/linux/x");
    }
}

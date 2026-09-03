//! SSH/SFTP 远程文件操作模块

use ssh2::Session;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

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
    #[error("TCP 连接失败: {0}")]
    TcpConnect(#[from] std::io::Error),
    #[error("SSH 操作失败: {0}")]
    Ssh(#[from] ssh2::Error),
    #[error("文件读取编码错误: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("命令执行失败 (exit={exit_code}): {output}")]
    CommandFailed { exit_code: i32, output: String },
    #[error("认证失败: {0}")]
    AuthFailed(String),
}

/// SSH keepalive 间隔（秒）
///
/// 每隔此间隔发送一次 SSH keepalive 包，防止空闲连接被服务器或中间网络设备（NAT/防火墙）超时断开。
const KEEPALIVE_INTERVAL_SECS: u32 = 30;

/// 公钥认证：ssh-agent 全部身份 → 私钥文件逐个尝试；全部失败时汇总每一步的原因
fn authenticate_public_key(
    session: &Session,
    username: &str,
    identity_files: &[PathBuf],
) -> Result<(), SshError> {
    let mut failures: Vec<String> = Vec::new();

    match authenticate_with_agent(session, username) {
        Ok(()) => return Ok(()),
        Err(reason) => failures.push(reason),
    }

    for key in identity_files {
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

/// 用 ssh-agent 中的每一个身份依次尝试认证；agent 不可用或全部被拒时返回原因
///
/// ssh2 自带的 `userauth_agent` 只会尝试第一个身份，故这里手动遍历。
fn authenticate_with_agent(session: &Session, username: &str) -> Result<(), String> {
    let mut agent = session
        .agent()
        .map_err(|e| format!("ssh-agent 不可用: {e}"))?;
    agent
        .connect()
        .map_err(|e| format!("ssh-agent 连接失败: {e}"))?;
    agent
        .list_identities()
        .map_err(|e| format!("ssh-agent 列举身份失败: {e}"))?;
    let identities = agent
        .identities()
        .map_err(|e| format!("ssh-agent 读取身份失败: {e}"))?;
    if identities.is_empty() {
        return Err("ssh-agent 中没有已加载的身份".into());
    }

    let mut last_error = String::new();
    for identity in &identities {
        match agent.userauth(username, identity) {
            Ok(()) => return Ok(()),
            Err(e) => last_error = e.to_string(),
        }
    }
    Err(format!(
        "ssh-agent 中 {} 个身份均被拒绝（最后错误: {last_error}）",
        identities.len()
    ))
}

/// 封装 SSH 会话，提供文件操作接口
pub struct SshConnection {
    session: Session,
}

impl SshConnection {
    /// 建立 SSH 连接并认证
    pub fn connect(config: &SshConfig) -> Result<Self, SshError> {
        let addr = format!("{}:{}", config.host, config.port);
        let tcp = TcpStream::connect(&addr)?;
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;

        let mut session = Session::new()?;
        session.set_tcp_stream(tcp);
        session.handshake()?;

        match &config.auth {
            AuthMethod::Password(password) => {
                session.userauth_password(&config.username, password)?;
            }
            AuthMethod::PublicKey { identity_files } => {
                authenticate_public_key(&session, &config.username, identity_files)?;
            }
        }

        // 启用 SSH keepalive：定期发送心跳包，want_reply=true 以便检测对端是否存活
        session.set_keepalive(true, KEEPALIVE_INTERVAL_SECS);

        Ok(Self { session })
    }

    /// 发送 SSH keepalive 心跳包
    ///
    /// 返回下次应发送心跳的秒数。连接已断开时返回 Err。
    pub fn send_keepalive(&self) -> Result<u32, SshError> {
        Ok(self.session.keepalive_send()?)
    }

    /// 列出远程目录下所有 .json 文件名
    pub fn list_json_files(&self, dir: &str) -> Result<Vec<String>, SshError> {
        let sftp = self.session.sftp()?;
        let entries = sftp.readdir(Path::new(dir))?;

        let mut files: Vec<String> = entries
            .into_iter()
            .filter_map(|(path, _stat)| {
                let name = path.file_name()?.to_string_lossy().into_owned();
                if name.ends_with(".json") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();

        files.sort();
        Ok(files)
    }

    /// 读取远程文件内容
    pub fn read_file(&self, path: &str) -> Result<String, SshError> {
        let sftp = self.session.sftp()?;
        let mut file = sftp.open(Path::new(path))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(String::from_utf8(buf)?)
    }

    /// 写入远程文件（覆盖）
    pub fn write_file(&self, path: &str, content: &str) -> Result<(), SshError> {
        let sftp = self.session.sftp()?;
        let mut file = sftp.create(Path::new(path))?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }

    /// 重命名远程文件
    pub fn rename_file(&self, old_path: &str, new_path: &str) -> Result<(), SshError> {
        let sftp = self.session.sftp()?;
        sftp.rename(Path::new(old_path), Path::new(new_path), None)?;
        Ok(())
    }

    /// 执行远程命令并返回 stdout
    pub fn exec_command(&self, command: &str) -> Result<String, SshError> {
        let mut channel = self.session.channel_session()?;
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

//! SSH/SFTP 远程文件操作模块

use ssh2::Session;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;

/// SSH 认证方式
#[allow(dead_code)]
pub enum AuthMethod {
    /// 密码认证
    Password(String),
    /// 密钥文件认证（私钥路径，可选密码短语）
    KeyFile {
        private_key: String,
        passphrase: Option<String>,
    },
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
            AuthMethod::KeyFile {
                private_key,
                passphrase,
            } => {
                session.userauth_pubkey_file(
                    &config.username,
                    None,
                    Path::new(private_key),
                    passphrase.as_deref(),
                )?;
            }
        }

        Ok(Self { session })
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

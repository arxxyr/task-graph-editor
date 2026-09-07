//! Unix ssh-agent 传输：所有系统调用均为非阻塞，轮询共享取消标记与绝对期限。

use super::{MAX_AGENT_PACKET, check_budget};
use crate::ssh::SshCancellation;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) struct UnixAgent {
    stream: UnixStream,
    usable: bool,
}

impl UnixAgent {
    pub(super) fn connect(
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<Self, String> {
        let path = std::env::var_os("SSH_AUTH_SOCK").ok_or("SSH_AUTH_SOCK 未设置")?;
        Self::connect_path(Path::new(&path), cancellation, deadline)
    }

    fn connect_path(
        path: &Path,
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<Self, String> {
        check_budget(cancellation, deadline)?;
        let bytes = path.as_os_str().as_bytes();
        // sockaddr_un 是平台 C ABI 的纯字段结构；零初始化确保填充和结尾字节确定。
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
            return Err("ssh-agent 套接字路径为空、含零字节或过长".into());
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as libc::c_char;
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        {
            address.sun_len = std::mem::size_of_val(&address) as u8;
        }
        // 新 fd 立即移交 OwnedFd，后续任一失败均自动关闭。
        let descriptor = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
        // 不向应用随后启动的子进程泄露 agent 通道。
        if unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let stream = UnixStream::from(owned);
        stream
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        // connect 使用非阻塞 fd，Unix backlog 堵塞也不能卡住 worker。
        let result = unsafe {
            libc::connect(
                stream.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                std::mem::size_of_val(&address) as libc::socklen_t,
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINPROGRESS) => {
                    wait_ready(&stream, libc::POLLOUT, cancellation, deadline)?;
                    if let Some(error) = stream.take_error().map_err(|error| error.to_string())? {
                        return Err(error.to_string());
                    }
                }
                _ => return Err(error.to_string()),
            }
        }
        Ok(Self {
            stream,
            usable: true,
        })
    }

    pub(super) fn exchange(
        &mut self,
        request: &[u8],
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<Vec<u8>, String> {
        if !self.usable {
            return Err("ssh-agent 上一次协议交换未完成，必须重新连接".into());
        }
        if request.len() > MAX_AGENT_PACKET {
            return Err("ssh-agent 请求过大".into());
        }
        self.usable = false;
        self.write_all(
            &(request.len() as u32).to_be_bytes(),
            cancellation,
            deadline,
        )?;
        self.write_all(request, cancellation, deadline)?;
        let mut length = [0; 4];
        self.read_exact(&mut length, cancellation, deadline)?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_AGENT_PACKET {
            return Err("ssh-agent 响应长度无效".into());
        }
        let mut response = vec![0; length];
        self.read_exact(&mut response, cancellation, deadline)?;
        self.usable = true;
        Ok(response)
    }

    fn write_all(
        &mut self,
        mut bytes: &[u8],
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<(), String> {
        while !bytes.is_empty() {
            check_budget(cancellation, deadline)?;
            match self.stream.write(bytes) {
                Ok(0) => return Err("ssh-agent 已关闭写通道".into()),
                Ok(count) => bytes = &bytes[count..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    wait_ready(&self.stream, libc::POLLOUT, cancellation, deadline)?
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    }

    fn read_exact(
        &mut self,
        mut bytes: &mut [u8],
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<(), String> {
        while !bytes.is_empty() {
            check_budget(cancellation, deadline)?;
            match self.stream.read(bytes) {
                Ok(0) => return Err("ssh-agent 在响应完成前关闭连接".into()),
                Ok(count) => bytes = &mut bytes[count..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    wait_ready(&self.stream, libc::POLLIN, cancellation, deadline)?
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    }
}

fn wait_ready(
    stream: &UnixStream,
    events: libc::c_short,
    cancellation: &SshCancellation,
    deadline: Instant,
) -> Result<(), String> {
    loop {
        check_budget(cancellation, deadline)?;
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(50));
        let timeout_ms = wait.as_millis().max(1) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd: stream.as_raw_fd(),
            events,
            revents: 0,
        };
        // descriptor 只在本次调用使用，poll 最多阻塞 50ms，没有存活于函数外的裸指针。
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        match result {
            0 => {}
            n if n > 0 => return Ok(()),
            _ => {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error.to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn pair() -> (UnixAgent, UnixStream) {
        let (client, server) = UnixStream::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        (
            UnixAgent {
                stream: client,
                usable: true,
            },
            server,
        )
    }

    #[test]
    fn 无响应agent到期返回且无需后台线程() {
        let (mut client, _server) = pair();
        let started = Instant::now();
        let result = client.exchange(
            &[11],
            &SshCancellation::default(),
            started + Duration::from_millis(50),
        );
        assert!(result.unwrap_err().contains("超时"));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!client.usable);
        assert!(
            client
                .exchange(
                    &[11],
                    &SshCancellation::default(),
                    Instant::now() + Duration::from_secs(5)
                )
                .unwrap_err()
                .contains("必须重新连接")
        );
    }

    #[test]
    fn 收到请求后取消会唤醒等待中的agent读取() {
        let (mut client, mut server) = pair();
        let cancellation = SshCancellation::default();
        let cancelled = cancellation.clone();
        let (sent, received) = mpsc::channel();
        let helper = std::thread::spawn(move || {
            let mut request = [0; 5];
            server.read_exact(&mut request).unwrap();
            cancelled.cancel();
            sent.send(server).unwrap();
        });
        let result = client.exchange(
            &[11],
            &cancellation,
            Instant::now() + Duration::from_secs(5),
        );
        assert!(result.unwrap_err().contains("取消"));
        let _server = received.recv().unwrap();
        helper.join().unwrap();
    }

    #[test]
    fn 分段帧与提前断开不会丢字节或死循环() {
        let (mut client, mut server) = pair();
        let helper = std::thread::spawn(move || {
            let mut request = [0; 5];
            server.read_exact(&mut request).unwrap();
            for byte in [0, 0, 0, 5, 12, 0, 0, 0, 0] {
                server.write_all(&[byte]).unwrap();
            }
        });
        let response = client
            .exchange(
                &[11],
                &SshCancellation::default(),
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(response, [12, 0, 0, 0, 0]);
        helper.join().unwrap();
        let (mut client, server) = pair();
        drop(server);
        assert!(
            client
                .exchange(
                    &[11],
                    &SshCancellation::default(),
                    Instant::now() + Duration::from_secs(2)
                )
                .is_err()
        );
    }

    #[test]
    fn 响应超限在分配之前拒绝() {
        let (mut client, mut server) = pair();
        let helper = std::thread::spawn(move || {
            let mut request = [0; 5];
            server.read_exact(&mut request).unwrap();
            server
                .write_all(&((MAX_AGENT_PACKET + 1) as u32).to_be_bytes())
                .unwrap();
        });
        assert!(
            client
                .exchange(
                    &[11],
                    &SshCancellation::default(),
                    Instant::now() + Duration::from_secs(2)
                )
                .unwrap_err()
                .contains("长度")
        );
        helper.join().unwrap();
    }
}

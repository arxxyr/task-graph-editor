//! 可由 UI 线程直接中断的 SSH 套接字；不等待阻塞中的 worker 读取取消消息。

use super::{SshConfig, SshError};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CANCEL_POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Default)]
pub struct SshCancellation {
    inner: Arc<CancellationInner>,
}

#[derive(Default)]
struct CancellationInner {
    cancelled: AtomicBool,
    socket: Mutex<Option<TcpStream>>,
}

impl SshCancellation {
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        if let Ok(socket) = self.inner.socket.lock()
            && let Some(socket) = socket.as_ref()
        {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub(super) fn check(&self) -> Result<(), SshError> {
        match self.is_cancelled() {
            true => Err(SshError::Cancelled),
            false => Ok(()),
        }
    }

    fn register(&self, socket: &TcpStream) -> Result<(), SshError> {
        let mut registered = self
            .inner
            .socket
            .lock()
            .map_err(|_| SshError::ConnectionState("取消控制器锁已损坏".into()))?;
        self.check()?;
        *registered = Some(socket.try_clone()?);
        Ok(())
    }
}

pub(super) fn connect_tcp(
    config: &SshConfig,
    cancellation: &SshCancellation,
) -> Result<TcpStream, SshError> {
    cancellation.check()?;
    // 系统 DNS 解析没有可移植的超时 API；隔离解析线程，worker 等待仍受统一期限与取消约束。
    let (sender, receiver) = mpsc::sync_channel(1);
    let host = config.host.clone();
    let port = config.port;
    std::thread::Builder::new()
        .name("ssh-dns".into())
        .spawn(move || {
            let result = (host.as_str(), port)
                .to_socket_addrs()
                .map(|addresses| addresses.collect::<Vec<_>>());
            let _ = sender.send(result);
        })?;
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let addresses = loop {
        cancellation.check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(timeout());
        }
        match receiver.recv_timeout(CANCEL_POLL.min(remaining)) {
            Ok(result) => break result?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(SshError::ConnectionState("DNS 解析线程异常退出".into()));
            }
        }
    };
    if addresses.is_empty() {
        return Err(SshError::ConnectionState("主机名未解析到任何地址".into()));
    }
    // 建连在独立线程内采用同一总期限，worker 可以立即取消；迟到的 socket 随结果销毁。
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("ssh-tcp".into())
        .spawn(move || {
            let mut result = Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "TCP 建连超时",
            ));
            for address in addresses {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                result = TcpStream::connect_timeout(&address, remaining);
                if result.is_ok() {
                    break;
                }
            }
            let _ = sender.send(result);
        })?;
    loop {
        cancellation.check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(timeout());
        }
        match receiver.recv_timeout(CANCEL_POLL.min(remaining)) {
            Ok(result) => {
                let socket = result?;
                cancellation.register(&socket)?;
                return Ok(socket);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(SshError::ConnectionState("TCP 建连线程异常退出".into()));
            }
        }
    }
}

fn timeout() -> SshError {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "SSH 地址解析或 TCP 建连超过 10 秒",
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn 取消直接关闭已注册套接字() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let cancellation = SshCancellation::default();
        cancellation.register(&socket).unwrap();
        cancellation.clone().cancel();
        assert!(cancellation.is_cancelled());
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        assert!(matches!(
            cancellation.register(&socket),
            Err(SshError::Cancelled)
        ));
    }
}

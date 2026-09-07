//! 可取消、有绝对期限的 ssh-agent 协议和 libssh2 公钥签名适配。
//!
//! 不调用 ssh2::Agent：上游 Unix socket、Windows Pageant/pipe 均会绕过 Session 超时。
//! 协议与传输使用安全 Rust；唯一 C 边界位于 signer_callback/authenticate_identity。

use super::SshCancellation;
use ssh2::Session;
use std::ffi::{CString, c_char, c_int, c_void};
use std::time::{Duration, Instant};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix::UnixAgent as AgentTransport;
#[cfg(windows)]
use windows::WindowsAgent as AgentTransport;

const MAX_AGENT_PACKET: usize = 1024 * 1024;
const MAX_AGENT_KEYS: usize = 1024;
const AGENT_TIMEOUT: Duration = Duration::from_secs(30);

fn check_budget(cancellation: &SshCancellation, deadline: Instant) -> Result<(), String> {
    match (cancellation.is_cancelled(), Instant::now() >= deadline) {
        (true, _) => Err("ssh-agent 操作已取消".into()),
        (_, true) => Err("ssh-agent 操作超时".into()),
        _ => Ok(()),
    }
}

pub(super) fn authenticate(
    session: &Session,
    username: &str,
    cancellation: &SshCancellation,
) -> Result<(), String> {
    let deadline = Instant::now() + AGENT_TIMEOUT;
    let mut transport = AgentTransport::connect(cancellation, deadline)?;
    let response = transport.exchange(&[11], cancellation, deadline)?;
    let identities = parse_identities(&response)?;
    attempt_identities(&identities, cancellation, deadline, |key| {
        authenticate_identity(
            session,
            username,
            key,
            &mut transport,
            cancellation,
            deadline,
        )
    })
}

fn attempt_identities(
    identities: &[Vec<u8>],
    cancellation: &SshCancellation,
    deadline: Instant,
    mut authenticate: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    if identities.is_empty() {
        return Err("ssh-agent 中没有已加载的身份".into());
    }
    let mut failures = Vec::new();
    for (index, identity) in identities.iter().enumerate() {
        check_budget(cancellation, deadline)?;
        match authenticate(identity) {
            Ok(()) => return Ok(()),
            Err(reason) => failures.push(format!("身份 {}: {reason}", index + 1)),
        }
    }
    Err(format!(
        "ssh-agent 身份均未通过认证（{}）",
        failures.join("；")
    ))
}

struct PacketReader<'a> {
    remaining: &'a [u8],
}

impl<'a> PacketReader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let (byte, rest) = self.remaining.split_first().ok_or("ssh-agent 响应被截断")?;
        self.remaining = rest;
        Ok(*byte)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| "ssh-agent 长度字段无效")?;
        Ok(u32::from_be_bytes(bytes))
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        if length > self.remaining.len() {
            return Err("ssh-agent 数据长度超出响应".into());
        }
        let (bytes, rest) = self.remaining.split_at(length);
        self.remaining = rest;
        Ok(bytes)
    }

    fn string(&mut self) -> Result<&'a [u8], String> {
        let length = self.u32()? as usize;
        self.take(length)
    }

    fn finish(self) -> Result<(), String> {
        match self.remaining.is_empty() {
            true => Ok(()),
            false => Err("ssh-agent 响应含多余数据".into()),
        }
    }
}

fn append_string(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_AGENT_PACKET || output.len() + 4 + bytes.len() > MAX_AGENT_PACKET {
        return Err("ssh-agent 请求过大".into());
    }
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(bytes);
    Ok(())
}

fn parse_identities(response: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if response.len() > MAX_AGENT_PACKET {
        return Err("ssh-agent 响应过大".into());
    }
    let mut reader = PacketReader {
        remaining: response,
    };
    if reader.byte()? != 12 {
        return Err("ssh-agent 拒绝列举身份".into());
    }
    let count = reader.u32()? as usize;
    if count > MAX_AGENT_KEYS {
        return Err("ssh-agent 返回的身份数量过多".into());
    }
    let mut identities = Vec::new();
    for _ in 0..count {
        let key = reader.string()?;
        if key.is_empty() {
            return Err("ssh-agent 返回空公钥".into());
        }
        let _comment = reader.string()?;
        identities.push(key.to_vec());
    }
    reader.finish()?;
    Ok(identities)
}

/// 签名方法来自 libssh2 构造的 RFC 4252 请求，保留 RSA SHA2 协商结果。
fn signing_method(data: &[u8]) -> Result<&[u8], String> {
    let mut reader = PacketReader { remaining: data };
    let _session_id = reader.string()?;
    if reader.byte()? != 50 {
        return Err("libssh2 签名请求不是用户认证消息".into());
    }
    let _username = reader.string()?;
    if reader.string()? != b"ssh-connection"
        || reader.string()? != b"publickey"
        || reader.byte()? != 1
    {
        return Err("libssh2 公钥签名请求结构无效".into());
    }
    let method = reader.string()?;
    let _public_key = reader.string()?;
    reader.finish()?;
    Ok(method)
}

fn plain_signing_method(method: &[u8]) -> &[u8] {
    match method {
        b"sk-ecdsa-sha2-nistp256-cert-v01@openssh.com" => b"sk-ecdsa-sha2-nistp256@openssh.com",
        b"sk-ssh-ed25519-cert-v01@openssh.com" => b"sk-ssh-ed25519@openssh.com",
        _ => method
            .strip_suffix(b"-cert-v01@openssh.com")
            .unwrap_or(method),
    }
}

fn signature_flags(method: &[u8]) -> u32 {
    match plain_signing_method(method) {
        b"rsa-sha2-256" => 2,
        b"rsa-sha2-512" => 4,
        _ => 0,
    }
}

#[derive(Debug, thiserror::Error)]
enum SignFailure {
    #[error("ssh-agent 返回的签名算法与请求不符")]
    AlgorithmMismatch,
    #[error("{0}")]
    Protocol(String),
}

impl From<String> for SignFailure {
    fn from(message: String) -> Self {
        Self::Protocol(message)
    }
}

impl From<&str> for SignFailure {
    fn from(message: &str) -> Self {
        Self::Protocol(message.to_owned())
    }
}

impl SignFailure {
    fn error_code(&self) -> c_int {
        match self {
            // 与 libssh2_agent_userauth 的语义一致，让 libssh2 按服务器能力尝试兼容签名方法。
            Self::AlgorithmMismatch => -51,
            Self::Protocol(_) => -42,
        }
    }
}

fn parse_signature(response: &[u8], expected_method: &[u8]) -> Result<Vec<u8>, SignFailure> {
    if response.len() > MAX_AGENT_PACKET {
        return Err("ssh-agent 响应过大".into());
    }
    let mut reader = PacketReader {
        remaining: response,
    };
    if reader.byte()? != 14 {
        return Err("ssh-agent 拒绝签名".into());
    }
    let signature = reader.string()?;
    reader.finish()?;
    let mut reader = PacketReader {
        remaining: signature,
    };
    let actual_method = reader.string()?;
    if actual_method != expected_method && actual_method != plain_signing_method(expected_method) {
        return Err(SignFailure::AlgorithmMismatch);
    }
    let signature = reader.string()?;
    if signature.is_empty() {
        return Err("ssh-agent 返回空签名".into());
    }
    reader.finish()?;
    Ok(signature.to_vec())
}

struct SignContext<'a> {
    transport: &'a mut AgentTransport,
    key: &'a [u8],
    cancellation: &'a SshCancellation,
    deadline: Instant,
    error: Option<String>,
}

impl SignContext<'_> {
    fn sign(&mut self, data: &[u8]) -> Result<Vec<u8>, SignFailure> {
        check_budget(self.cancellation, self.deadline)?;
        let method = signing_method(data)?;
        let mut request = vec![13];
        append_string(&mut request, self.key)?;
        append_string(&mut request, data)?;
        request.extend_from_slice(&signature_flags(method).to_be_bytes());
        if request.len() > MAX_AGENT_PACKET {
            return Err("ssh-agent 签名请求过大".into());
        }
        let response = self
            .transport
            .exchange(&request, self.cancellation, self.deadline)?;
        parse_signature(&response, method)
    }
}

type SignCallback = unsafe extern "C" fn(
    *mut c_void,
    *mut *mut u8,
    *mut usize,
    *const u8,
    usize,
    *mut *mut c_void,
) -> c_int;

// 签名和 ABI 来自项目锁定的 libssh2.h；ssh2 尚未封装此公开 API。
unsafe extern "C" {
    fn libssh2_userauth_publickey(
        session: *mut c_void,
        username: *const c_char,
        key: *const u8,
        key_length: usize,
        callback: SignCallback,
        context: *mut *mut c_void,
    ) -> c_int;
}

unsafe extern "C" fn signer_callback(
    _session: *mut c_void,
    signature: *mut *mut u8,
    signature_length: *mut usize,
    data: *const u8,
    data_length: usize,
    context: *mut *mut c_void,
) -> c_int {
    if signature.is_null()
        || signature_length.is_null()
        || data.is_null()
        || context.is_null()
        || data_length > MAX_AGENT_PACKET
    {
        return -42;
    }
    unsafe {
        *signature = std::ptr::null_mut();
        *signature_length = 0;
    }
    // context 由 authenticate_identity 的栈帧拥有；C 仅在同步调用期间借用，回调返回后不保留。
    let context_pointer = unsafe { *context };
    if context_pointer.is_null() {
        return -42;
    }
    let context = unsafe { &mut *context_pointer.cast::<SignContext<'_>>() };
    let data = unsafe { std::slice::from_raw_parts(data, data_length) };
    // 任何 Rust panic 都在 C 边界内截住，绝不跨过 libssh2 栈帧。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| context.sign(data)));
    let result = match result {
        Ok(result) => result,
        Err(_) => Err("ssh-agent 签名适配器异常".into()),
    };
    match result {
        Ok(bytes) => {
            // Session::new 使用 libssh2 默认 malloc/free；缓冲由同一个 C 分配器分配并交给 libssh2 free。
            // Rust Vec 只作源数据，绝不将 Rust 分配的内存交给 C 释放。
            let allocated = unsafe { libc::malloc(bytes.len()) }.cast::<u8>();
            if allocated.is_null() {
                return -6;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), allocated, bytes.len());
                *signature = allocated;
                *signature_length = bytes.len();
            }
            0
        }
        Err(error) => {
            let code = error.error_code();
            context.error = Some(error.to_string());
            code
        }
    }
}

/// 每个身份的网络认证也受剩余总期限约束，退出作用域后恢复普通 SSH 操作的 API 超时。
struct SessionTimeoutGuard<'a> {
    session: &'a Session,
    previous: u32,
}

impl<'a> SessionTimeoutGuard<'a> {
    fn new(session: &'a Session, deadline: Instant) -> Result<Self, String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("ssh-agent 操作超时".into());
        }
        let previous = session.timeout();
        let remaining_ms = remaining.as_millis().clamp(1, u128::from(u32::MAX)) as u32;
        session.set_timeout(match previous {
            0 => remaining_ms,
            timeout => timeout.min(remaining_ms),
        });
        Ok(Self { session, previous })
    }
}

impl Drop for SessionTimeoutGuard<'_> {
    fn drop(&mut self) {
        self.session.set_timeout(self.previous);
    }
}

fn authenticate_identity(
    session: &Session,
    username: &str,
    key: &[u8],
    transport: &mut AgentTransport,
    cancellation: &SshCancellation,
    deadline: Instant,
) -> Result<(), String> {
    check_budget(cancellation, deadline)?;
    let username = CString::new(username).map_err(|_| "SSH 用户名含零字节")?;
    // 必须先创建 guard 再锁 raw：逆序析构保证恢复 timeout 时不重入仍持有的会话锁。
    let _timeout_guard = SessionTimeoutGuard::new(session, deadline)?;
    let mut context = SignContext {
        transport,
        key,
        cancellation,
        deadline,
        error: None,
    };
    let mut context_pointer = (&mut context as *mut SignContext<'_>).cast::<c_void>();
    // raw() 持有会话互斥锁直至 C 返回；回调只使用独立 agent 传输，不重入 Session。
    let mut raw = session.raw();
    let session_pointer = std::ptr::from_mut(&mut *raw);
    let result = unsafe {
        libssh2_userauth_publickey(
            session_pointer.cast(),
            username.as_ptr(),
            key.as_ptr(),
            key.len(),
            signer_callback,
            &mut context_pointer,
        )
    };
    match result {
        0 => Ok(()),
        code => Err(context.error.unwrap_or_else(|| {
            ssh2::Error::from_session_error_raw(session_pointer, code).to_string()
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 单身份网络超时夹到剩余期限并恢复原配置() {
        let session = Session::new().unwrap();
        session.set_timeout(30_000);
        {
            let _guard =
                SessionTimeoutGuard::new(&session, Instant::now() + Duration::from_millis(100))
                    .unwrap();
            assert!((1..=100).contains(&session.timeout()));
        }
        assert_eq!(session.timeout(), 30_000);
        assert!(SessionTimeoutGuard::new(&session, Instant::now()).is_err());
        assert_eq!(session.timeout(), 30_000);
    }

    #[test]
    fn 身份响应保留顺序并拒绝截断超限和尾部数据() {
        let mut packet = vec![12, 0, 0, 0, 2];
        for key in [b"first".as_slice(), b"second".as_slice()] {
            append_string(&mut packet, key).unwrap();
            append_string(&mut packet, b"comment").unwrap();
        }
        assert_eq!(
            parse_identities(&packet).unwrap(),
            [b"first".to_vec(), b"second".to_vec()]
        );
        assert!(parse_identities(&packet[..packet.len() - 1]).is_err());
        packet.push(0);
        assert!(parse_identities(&packet).is_err());
        assert!(parse_identities(&[12, 255, 255, 255, 255]).is_err());
        assert!(parse_identities(&[5]).is_err());
    }

    #[test]
    fn 身份逐个尝试直到成功且失败原因完整汇总() {
        let identities = vec![vec![1], vec![2], vec![3]];
        let mut visited = Vec::new();
        let cancellation = SshCancellation::default();
        let deadline = Instant::now() + Duration::from_secs(2);
        assert!(
            attempt_identities(&identities, &cancellation, deadline, |key| {
                visited.push(key[0]);
                match key[0] {
                    2 => Ok(()),
                    _ => Err("拒绝".into()),
                }
            })
            .is_ok()
        );
        assert_eq!(visited, [1, 2]);
        let error =
            attempt_identities(&identities, &cancellation, deadline, |_| Err("拒绝".into()))
                .unwrap_err();
        assert!(error.contains("身份 1") && error.contains("身份 2") && error.contains("身份 3"));
    }

    #[test]
    fn 身份之间响应取消且不再调用后续身份() {
        let cancellation = SshCancellation::default();
        let mut calls = 0;
        let result = attempt_identities(
            &[vec![1], vec![2]],
            &cancellation,
            Instant::now() + Duration::from_secs(2),
            |_| {
                calls += 1;
                cancellation.cancel();
                Err("拒绝".into())
            },
        );
        assert!(result.unwrap_err().contains("取消"));
        assert_eq!(calls, 1);
    }

    fn sign_response(method: &[u8], signature: &[u8]) -> Vec<u8> {
        let mut inner = Vec::new();
        append_string(&mut inner, method).unwrap();
        append_string(&mut inner, signature).unwrap();
        let mut response = vec![14];
        append_string(&mut response, &inner).unwrap();
        response
    }

    #[test]
    fn 签名算法和结构必须与请求一致() {
        assert_eq!(signature_flags(b"rsa-sha2-256"), 2);
        assert_eq!(signature_flags(b"rsa-sha2-512"), 4);
        assert_eq!(signature_flags(b"rsa-sha2-512-cert-v01@openssh.com"), 4);
        assert!(
            parse_signature(
                &sign_response(b"ssh-ed25519", b"signature"),
                b"ssh-ed25519-cert-v01@openssh.com"
            )
            .is_ok()
        );
        assert_eq!(signature_flags(b"ssh-ed25519"), 0);
        let response = sign_response(b"rsa-sha2-512", b"signature");
        assert_eq!(
            parse_signature(&response, b"rsa-sha2-512").unwrap(),
            b"signature"
        );
        assert!(parse_signature(&response, b"ssh-rsa").is_err());
        assert!(parse_signature(&sign_response(b"ssh-rsa", b""), b"ssh-rsa").is_err());
        assert!(parse_signature(&[5], b"ssh-rsa").is_err());
    }

    #[test]
    fn 从认证报文获取算法而非从公钥猜测rsa签名方案() {
        let mut data = Vec::new();
        append_string(&mut data, b"session").unwrap();
        data.push(50);
        for text in [b"user".as_slice(), b"ssh-connection", b"publickey"] {
            append_string(&mut data, text).unwrap();
        }
        data.push(1);
        append_string(&mut data, b"rsa-sha2-512").unwrap();
        append_string(&mut data, b"ssh-rsa-key").unwrap();
        assert_eq!(signing_method(&data).unwrap(), b"rsa-sha2-512");
        assert!(signing_method(&data[..data.len() - 1]).is_err());
    }
}

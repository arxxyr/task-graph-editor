//! Windows agent 通道：Pageant 使用有界窗口消息，OpenSSH 使用可取消的重叠管道 I/O。

use std::ffi::CString;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_SEM_TIMEOUT,
    GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HWND, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE,
    UnmapViewOfFile,
};
use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SMTO_ABORTIFHUNG, SMTO_BLOCK, SMTO_ERRORONEXIT, SendMessageTimeoutW, WM_COPYDATA,
};

use super::MAX_AGENT_PACKET;
use crate::ssh::SshCancellation;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const PAGEANT_PACKET_SIZE: usize = 8192;
const PAGEANT_COPYDATA_ID: usize = 0x804e50ba;
const REQUEST_IDENTITIES: u8 = 11;
const IDENTITIES_ANSWER: u8 = 12;
static NEXT_MAPPING: AtomicU64 = AtomicU64::new(0);

pub(super) struct WindowsAgent {
    transport: Transport,
}

enum Transport {
    Pageant,
    Pipe(NamedPipe),
}

impl WindowsAgent {
    pub(super) fn connect(
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<Self, String> {
        remaining(cancellation, deadline)?;
        let transport = match pageant_window().is_null() {
            false => Transport::Pageant,
            true => Transport::Pipe(NamedPipe::connect(cancellation, deadline)?),
        };
        Ok(Self { transport })
    }

    pub(super) fn exchange(
        &mut self,
        request: &[u8],
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<Vec<u8>, String> {
        remaining(cancellation, deadline)?;
        if request.is_empty() || request.len() > MAX_AGENT_PACKET {
            return Err("agent 请求长度无效".into());
        }
        match &mut self.transport {
            Transport::Pipe(pipe) => pipe.exchange(request, cancellation, deadline),
            Transport::Pageant => {
                let result = pageant_exchange(request, cancellation, deadline);
                // 身份枚举失败或为空时才切换来源；签名请求不重发，避免重复触发硬件确认。
                let fallback = request == [REQUEST_IDENTITIES]
                    && !result.as_ref().is_ok_and(|answer| {
                        answer.len() >= 5
                            && answer[0] == IDENTITIES_ANSWER
                            && answer[1..5] != [0, 0, 0, 0]
                    });
                if !fallback {
                    return result;
                }
                remaining(cancellation, deadline)?;
                let mut pipe = NamedPipe::connect(cancellation, deadline).map_err(|error| {
                    let previous = match &result {
                        Ok(_) => "Pageant 没有可用身份",
                        Err(reason) => reason.as_str(),
                    };
                    format!("{previous}；OpenSSH agent: {error}")
                })?;
                let answer = pipe.exchange(request, cancellation, deadline)?;
                self.transport = Transport::Pipe(pipe);
                Ok(answer)
            }
        }
    }
}

fn remaining(cancellation: &SshCancellation, deadline: Instant) -> Result<Duration, String> {
    if cancellation.is_cancelled() {
        return Err("agent 操作已取消".into());
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    match remaining.is_zero() {
        true => Err("agent 操作超时".into()),
        false => Ok(remaining),
    }
}

fn milliseconds(duration: Duration) -> u32 {
    // Win32 的 0 表示不等待，正数微秒必须向上保留至少 1ms。
    duration.as_millis().clamp(1, u128::from(u32::MAX - 1)) as u32
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn system_error(operation: &str) -> String {
    format!("{operation}: {}", std::io::Error::last_os_error())
}

/// 只包装真正拥有的内核句柄，不接受伪句柄；析构不等待远端服务。
struct OwnedHandle(HANDLE);

// 安全：这里只拥有真实内核句柄，Windows 允许在线程之间转移；不提供 Sync，
// 同一管道的请求仍由单一所有者串行执行，不跨线程共享 OVERLAPPED 或缓冲区。
unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    fn new(handle: HANDLE, operation: &str) -> Result<Self, String> {
        match handle.is_null() || handle == INVALID_HANDLE_VALUE {
            true => Err(system_error(operation)),
            false => Ok(Self(handle)),
        }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // 安全：句柄来自成功的 Create*，独占所有权，恰好关闭一次。
        unsafe { CloseHandle(self.0) };
    }
}

fn pageant_window() -> HWND {
    let name = wide("Pageant");
    // 安全：名称以 NUL 结尾，调用期间缓冲保持有效；返回值只是借用的窗口句柄。
    unsafe { FindWindowW(name.as_ptr(), name.as_ptr()) }
}

fn pageant_exchange(
    request: &[u8],
    cancellation: &SshCancellation,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    if request.len() > PAGEANT_PACKET_SIZE - 4 {
        return Err("请求超过 Pageant 的 8192 字节消息容量".into());
    }
    remaining(cancellation, deadline)?;
    let request = request.to_vec();
    let cancellation_worker = cancellation.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("ssh-pageant".into())
        .spawn(move || {
            let result = pageant_exchange_blocking(&request, &cancellation_worker, deadline);
            let _ = sender.send(result);
        })
        .map_err(|error| format!("无法启动 Pageant 消息线程: {error}"))?;
    loop {
        let wait = remaining(cancellation, deadline)?.min(POLL_INTERVAL);
        match receiver.recv_timeout(wait) {
            Ok(result) => {
                worker
                    .join()
                    .map_err(|_| "Pageant 消息线程异常退出".to_owned())?;
                remaining(cancellation, deadline)?;
                return result;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Pageant 消息线程异常退出".into());
            }
        }
    }
    // 取消时辅助线程最多存活到同一 deadline；唯一阻塞调用有明确超时。
    // 映射与请求由该线程独占持有，取消返回不会提前释放 Pageant 正在读取的内存。
}

/// 映射视图必须先于映射句柄关闭；只在创建它的消息线程使用。
struct SharedMapping {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    _handle: OwnedHandle,
}

impl SharedMapping {
    fn create(name: &str) -> Result<Self, String> {
        let name = wide(name);
        // 安全：匿名页文件映射、默认当前用户 DACL，名称和容量固定有效。
        let raw = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                null(),
                PAGE_READWRITE,
                0,
                PAGEANT_PACKET_SIZE as u32,
                name.as_ptr(),
            )
        };
        // 必须紧跟 CreateFileMapping 读取，后续 API 会改写线程错误码。
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let handle = OwnedHandle::new(raw, "创建 Pageant 共享映射失败")?;
        if existed {
            return Err("Pageant 共享映射名称冲突，拒绝复用其他进程的映射".into());
        }
        // 安全：映射句柄有效，映射大小与创建容量一致。
        let view = unsafe { MapViewOfFile(handle.0, FILE_MAP_WRITE, 0, 0, PAGEANT_PACKET_SIZE) };
        if view.Value.is_null() {
            return Err(system_error("映射 Pageant 共享内存失败"));
        }
        Ok(Self {
            view,
            _handle: handle,
        })
    }

    fn write_request(&mut self, request: &[u8]) {
        let pointer = self.view.Value.cast::<u8>();
        let length = (request.len() as u32).to_be_bytes();
        // 安全：调用方已校验长度，当前线程独占写入，Pageant 尚未收到消息。
        unsafe {
            std::ptr::copy_nonoverlapping(length.as_ptr(), pointer, 4);
            std::ptr::copy_nonoverlapping(request.as_ptr(), pointer.add(4), request.len());
        }
    }

    fn response(&self) -> Result<Vec<u8>, String> {
        let pointer = self.view.Value.cast::<u8>();
        // 外部进程改写共享内存，逐字节 volatile 读取，不建立可跨外部写入的 Rust 引用。
        let length = u32::from_be_bytes(std::array::from_fn(|index| unsafe {
            pointer.add(index).read_volatile()
        })) as usize;
        if length == 0 || length > (PAGEANT_PACKET_SIZE - 4).min(MAX_AGENT_PACKET) {
            return Err("Pageant 响应长度无效".into());
        }
        let mut response = vec![0; length];
        for (index, byte) in response.iter_mut().enumerate() {
            // 安全：长度已验证在映射范围内，视图由 self 保持有效。
            *byte = unsafe { pointer.add(index + 4).read_volatile() };
        }
        Ok(response)
    }
}

impl Drop for SharedMapping {
    fn drop(&mut self) {
        // 安全：只解除本对象持有的有效视图；随后字段析构关闭映射句柄。
        unsafe { UnmapViewOfFile(self.view) };
    }
}

fn pageant_exchange_blocking(
    request: &[u8],
    cancellation: &SshCancellation,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    remaining(cancellation, deadline)?;
    let window = pageant_window();
    if window.is_null() {
        return Err("Pageant 未运行".into());
    }
    let sequence = NEXT_MAPPING.fetch_add(1, Ordering::Relaxed);
    let name = format!("PageantRequest{:08x}{sequence:016x}", std::process::id());
    let mut mapping = SharedMapping::create(&name)?;
    mapping.write_request(request);
    let name = CString::new(name).map_err(|_| "Pageant 映射名称无效".to_owned())?;
    let message = COPYDATASTRUCT {
        dwData: PAGEANT_COPYDATA_ID,
        cbData: name.as_bytes_with_nul().len() as u32,
        lpData: name.as_ptr().cast_mut().cast(),
    };
    let timeout = milliseconds(remaining(cancellation, deadline)?);
    let mut response = 0;
    // 安全：WM_COPYDATA 由系统跨进程封送；name、message、mapping 均活到调用返回。
    // 不使用 SMTO_NOTIMEOUTIFNOTHUNG，活跃但不完成签名的 Pageant 也必须遵守期限。
    let accepted = unsafe {
        SendMessageTimeoutW(
            window,
            WM_COPYDATA,
            0,
            (&message as *const COPYDATASTRUCT) as isize,
            SMTO_BLOCK | SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
            timeout,
            &mut response,
        )
    };
    remaining(cancellation, deadline)?;
    if accepted == 0 || response == 0 {
        return Err("Pageant 未完成请求或窗口消息超时".into());
    }
    mapping.response()
}

struct NamedPipe {
    handle: OwnedHandle,
    usable: bool,
}

impl NamedPipe {
    fn connect(cancellation: &SshCancellation, deadline: Instant) -> Result<Self, String> {
        let path =
            std::env::var("SSH_AUTH_SOCK").unwrap_or_else(|_| r"\\.\pipe\openssh-ssh-agent".into());
        Self::connect_path(&path, cancellation, deadline)
    }

    fn connect_path(
        path: &str,
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<Self, String> {
        // 仅允许本机管道，不能让 CreateFile 意外进入不可控的远程 SMB 建连。
        if !path.to_ascii_lowercase().starts_with(r"\\.\pipe\") || path.contains('\0') {
            return Err("SSH_AUTH_SOCK 必须指向本机 Windows named pipe".into());
        }
        let path = wide(path);
        loop {
            remaining(cancellation, deadline)?;
            // 安全：NUL 结尾本机管道名，不共享所有权，所有 I/O 强制走 OVERLAPPED。
            let raw = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    null_mut(),
                )
            };
            if raw != INVALID_HANDLE_VALUE {
                return Ok(Self {
                    handle: OwnedHandle::new(raw, "打开 OpenSSH 管道失败")?,
                    usable: true,
                });
            }
            let error = unsafe { GetLastError() };
            if error != ERROR_PIPE_BUSY {
                return Err(system_error("打开 OpenSSH 管道失败"));
            }
            let wait = remaining(cancellation, deadline)?.min(POLL_INTERVAL);
            // 安全：等待本机管道实例，单次最多 POLL_INTERVAL，循环复查取消与绝对期限。
            let available = unsafe { WaitNamedPipeW(path.as_ptr(), milliseconds(wait)) };
            if available == 0 && unsafe { GetLastError() } != ERROR_SEM_TIMEOUT {
                return Err(system_error("等待 OpenSSH 管道失败"));
            }
        }
    }

    fn exchange(
        &mut self,
        request: &[u8],
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<Vec<u8>, String> {
        if !self.usable {
            return Err("agent 管道上一次请求未完成，必须重新连接".into());
        }
        self.usable = false;
        let mut packet = Vec::with_capacity(request.len() + 4);
        packet.extend_from_slice(&(request.len() as u32).to_be_bytes());
        packet.extend_from_slice(request);
        self.transfer_all(&mut packet, true, cancellation, deadline)?;
        let mut header = [0; 4];
        self.transfer_all(&mut header, false, cancellation, deadline)?;
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > MAX_AGENT_PACKET {
            return Err("OpenSSH agent 响应长度无效".into());
        }
        let mut answer = vec![0; length];
        self.transfer_all(&mut answer, false, cancellation, deadline)?;
        self.usable = true;
        Ok(answer)
    }

    fn transfer_all(
        &self,
        buffer: &mut [u8],
        write: bool,
        cancellation: &SshCancellation,
        deadline: Instant,
    ) -> Result<(), String> {
        let mut offset = 0;
        while offset < buffer.len() {
            remaining(cancellation, deadline)?;
            let mut io = PendingIo::new(&self.handle)?;
            let mut transferred = 0;
            // 安全：buffer 与堆上的 OVERLAPPED 在本次 I/O 完成或取消确认前均不会移动/释放。
            let completed = unsafe {
                match write {
                    true => WriteFile(
                        self.handle.0,
                        buffer[offset..].as_ptr(),
                        (buffer.len() - offset) as u32,
                        &mut transferred,
                        io.overlapped.as_mut(),
                    ),
                    false => ReadFile(
                        self.handle.0,
                        buffer[offset..].as_mut_ptr(),
                        (buffer.len() - offset) as u32,
                        &mut transferred,
                        io.overlapped.as_mut(),
                    ),
                }
            };
            match completed != 0 {
                true => {}
                false => {
                    if unsafe { GetLastError() } != ERROR_IO_PENDING {
                        return Err(system_error("OpenSSH agent 管道 I/O 失败"));
                    }
                    io.pending = true;
                    transferred = io.wait(cancellation, deadline)?;
                }
            }
            if transferred == 0 {
                return Err("OpenSSH agent 关闭了管道".into());
            }
            offset += transferred as usize;
        }
        Ok(())
    }
}

/// 取消 I/O 后必须确认内核不再引用缓冲区，不能仅发 CancelIoEx 就释放 OVERLAPPED。
struct PendingIo<'a> {
    pipe: &'a OwnedHandle,
    overlapped: Box<OVERLAPPED>,
    event: OwnedHandle,
    pending: bool,
}

impl<'a> PendingIo<'a> {
    fn new(pipe: &'a OwnedHandle) -> Result<Self, String> {
        // 安全：未命名手动重置事件，仅此 I/O 使用。
        let event = OwnedHandle::new(
            unsafe { CreateEventW(null(), 1, 0, null()) },
            "创建 I/O 事件失败",
        )?;
        let overlapped = Box::new(OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        });
        Ok(Self {
            pipe,
            overlapped,
            event,
            pending: false,
        })
    }

    fn wait(&mut self, cancellation: &SshCancellation, deadline: Instant) -> Result<u32, String> {
        loop {
            let wait = remaining(cancellation, deadline)?.min(POLL_INTERVAL);
            // 安全：事件与 I/O 均保持存活，等待时不释放任何缓冲。
            match unsafe { WaitForSingleObject(self.event.0, milliseconds(wait)) } {
                WAIT_OBJECT_0 => {
                    let mut transferred = 0;
                    // 安全：事件已置位，查询不阻塞，OVERLAPPED 属于这个 pipe。
                    let completed = unsafe {
                        GetOverlappedResult(
                            self.pipe.0,
                            self.overlapped.as_ref(),
                            &mut transferred,
                            0,
                        )
                    };
                    self.pending = false;
                    return match completed != 0 {
                        true => Ok(transferred),
                        false => Err(system_error("OpenSSH agent 管道 I/O 未完成")),
                    };
                }
                WAIT_TIMEOUT => {}
                _ => return Err(system_error("等待 OpenSSH agent 响应失败")),
            }
        }
    }
}

impl Drop for PendingIo<'_> {
    fn drop(&mut self) {
        if self.pending {
            let mut transferred = 0;
            // 安全：仅处理本机 NPFS 的可取消管道 I/O。CancelIoEx 后同步回收完成状态，
            // 避免取消请求与内核最后一次写 buffer/OVERLAPPED 竞态；不能提前释放栈上借用。
            unsafe {
                CancelIoEx(self.pipe.0, self.overlapped.as_ref());
                GetOverlappedResult(self.pipe.0, self.overlapped.as_ref(), &mut transferred, 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_PIPE_CONNECTED;
    use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE,
    };

    fn pipe_pair() -> (NamedPipe, NamedPipe) {
        let name = format!(
            r"\\.\pipe\tge-agent-test-{}-{}",
            std::process::id(),
            NEXT_MAPPING.fetch_add(1, Ordering::Relaxed),
        );
        let encoded = wide(&name);
        // 安全：只创建唯一的本地测试管道，拒绝远程客户端，不接触真实 agent。
        let handle = OwnedHandle::new(
            unsafe {
                CreateNamedPipeW(
                    encoded.as_ptr(),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    4096,
                    4096,
                    0,
                    null(),
                )
            },
            "创建测试管道失败",
        )
        .unwrap();
        let cancellation = SshCancellation::default();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut io = PendingIo::new(&handle).unwrap();
        // 安全：管道与 OVERLAPPED 在连接完成前由当前函数保持存活。
        let connected = unsafe { ConnectNamedPipe(handle.0, io.overlapped.as_mut()) };
        if connected == 0 {
            match unsafe { GetLastError() } {
                ERROR_IO_PENDING => io.pending = true,
                ERROR_PIPE_CONNECTED => {}
                code => panic!("测试管道连接失败: {code}"),
            }
        }
        let client = NamedPipe::connect_path(&name, &cancellation, deadline).unwrap();
        if io.pending {
            io.wait(&cancellation, deadline).unwrap();
        }
        drop(io);
        (
            client,
            NamedPipe {
                handle,
                usable: true,
            },
        )
    }

    #[test]
    fn 静默本地管道在期限内取消挂起读取() {
        let (mut client, _server) = pipe_pair();
        let cancellation = SshCancellation::default();
        let started = Instant::now();
        let error = client
            .exchange(
                &[REQUEST_IDENTITIES],
                &cancellation,
                started + Duration::from_millis(100),
            )
            .unwrap_err();
        assert!(error.contains("超时"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!client.usable, "部分协议交换后不能复用流状态");
    }

    #[test]
    fn 管道读到请求后取消无需服务端关闭连接() {
        let (mut client, server) = pipe_pair();
        let cancellation = SshCancellation::default();
        let cancel_peer = cancellation.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let server_thread = std::thread::spawn(move || {
            let mut request = [0; 5];
            server
                .transfer_all(
                    &mut request,
                    false,
                    &SshCancellation::default(),
                    Instant::now() + Duration::from_secs(3),
                )
                .unwrap();
            assert_eq!(request, [0, 0, 0, 1, REQUEST_IDENTITIES]);
            cancel_peer.cancel();
            // 保持服务端句柄存活到客户端退出，证明由取消而不是 EOF 唤醒。
            done_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        let error = client
            .exchange(
                &[REQUEST_IDENTITIES],
                &cancellation,
                Instant::now() + Duration::from_secs(3),
            )
            .unwrap_err();
        done_tx.send(()).unwrap();
        server_thread.join().unwrap();
        assert!(error.contains("取消"), "{error}");
    }

    #[test]
    fn 管道拒绝越界响应而不分配声明大小() {
        let (mut client, server) = pipe_pair();
        let server_thread = std::thread::spawn(move || {
            let cancellation = SshCancellation::default();
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut request = [0; 5];
            server
                .transfer_all(&mut request, false, &cancellation, deadline)
                .unwrap();
            let mut header = ((MAX_AGENT_PACKET + 1) as u32).to_be_bytes();
            server
                .transfer_all(&mut header, true, &cancellation, deadline)
                .unwrap();
        });
        let error = client
            .exchange(
                &[REQUEST_IDENTITIES],
                &SshCancellation::default(),
                Instant::now() + Duration::from_secs(3),
            )
            .unwrap_err();
        server_thread.join().unwrap();
        assert!(error.contains("响应长度无效"), "{error}");
    }

    #[test]
    fn 取消或过期时不会探测真实agent() {
        let cancellation = SshCancellation::default();
        cancellation.cancel();
        assert!(
            WindowsAgent::connect(&cancellation, Instant::now() + Duration::from_secs(1)).is_err()
        );
        assert!(WindowsAgent::connect(&SshCancellation::default(), Instant::now()).is_err());
    }

    #[test]
    fn pageant共享映射响应受容量约束() {
        let name = format!(
            "PageantTest{}-{}",
            std::process::id(),
            NEXT_MAPPING.fetch_add(1, Ordering::Relaxed)
        );
        let mut mapping = SharedMapping::create(&name).unwrap();
        let response = [IDENTITIES_ANSWER, 0, 0, 0, 0];
        mapping.write_request(&response);
        assert_eq!(mapping.response().unwrap(), response);
        let length = (PAGEANT_PACKET_SIZE as u32).to_be_bytes();
        // 安全：仅覆盖本测试拥有的映射头，不向外部 Pageant 发送消息。
        unsafe { std::ptr::copy_nonoverlapping(length.as_ptr(), mapping.view.Value.cast(), 4) };
        assert!(mapping.response().is_err());
    }
}

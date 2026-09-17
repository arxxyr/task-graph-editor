//! 使用当前 SSH 会话浏览和下载日志；本地文件独占写入，取消后删除未完成文件。

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{SshConnection, SshError};

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub name: String,
    pub directory: bool,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct LogListing {
    pub directory: String,
    pub entries: Vec<LogEntry>,
}

/// 文件名末两段是 PID 和 HHMMSS 加小数秒；PID 不参与创建时间排序。
fn log_creation_time(name: &str) -> Option<chrono::NaiveTime> {
    let stem = name.strip_suffix(".log")?;
    let (prefix_pid, time) = stem.rsplit_once('_')?;
    let (prefix, pid) = prefix_pid.rsplit_once('_')?;
    if prefix != "master_control"
        || pid.is_empty()
        || !pid.bytes().all(|c| c.is_ascii_digit())
        || !(6..=15).contains(&time.len())
        || !time.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let nanos = match &time[6..] {
        "" => 0,
        fraction => fraction.parse::<u32>().ok()? * 10_u32.pow(9 - fraction.len() as u32),
    };
    chrono::NaiveTime::from_hms_nano_opt(
        time[..2].parse().ok()?,
        time[2..4].parse().ok()?,
        time[4..6].parse().ok()?,
        nanos,
    )
}

/// 主控日志按真实日历日期归档，排除 ROS 启动目录及其他组件日志。
fn filter_master_control_entries(entries: &mut Vec<LogEntry>) {
    entries.retain(|entry| match entry.directory {
        true => {
            entry.name.len() == 8
                && entry.name.bytes().all(|c| c.is_ascii_digit())
                && chrono::NaiveDate::parse_from_str(&entry.name, "%Y%m%d").is_ok()
        }
        false => {
            (entry.name == "master_control.log" || entry.name.starts_with("master_control_"))
                && entry.name.ends_with(".log")
        }
    });
    // 缓存解析键；无法识别时间的旧文件排在末尾，不用 mtime 冒充创建时间。
    entries.sort_by_cached_key(|entry| {
        (
            std::cmp::Reverse(entry.directory),
            std::cmp::Reverse(
                match entry.directory {
                    true => entry.name.as_str(),
                    false => "",
                }
                .to_string(),
            ),
            std::cmp::Reverse(log_creation_time(&entry.name)),
            entry.name.clone(),
        )
    });
}

#[cfg(test)]
mod listing_tests {
    use super::*;

    #[test]
    fn 文件按创建时刻倒序而不是按进程号排序() {
        let mut entries: Vec<_> = [
            "master_control_999999_035802107526.log",
            "master_control_1_165416802673.log",
            "master_control_2_165416802674.log",
            "master_control_3_165416802.log",
            "master_control_4_165416.log",
            "master_control.log",
            "master_control_5_250000000000.log",
        ]
        .into_iter()
        .map(|name| LogEntry {
            name: name.into(),
            directory: false,
            size: 0,
        })
        .collect();
        filter_master_control_entries(&mut entries);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            [
                "master_control_2_165416802674.log",
                "master_control_1_165416802673.log",
                "master_control_3_165416802.log",
                "master_control_4_165416.log",
                "master_control_999999_035802107526.log",
                "master_control.log",
                "master_control_5_250000000000.log",
            ]
        );
        assert_eq!(
            log_creation_time("master_control_1_165416802.log"),
            log_creation_time("master_control_2_165416802000.log")
        );
        for name in [
            "master_control_x_165416.log",
            "mastercontrol_123_165416802673.log",
            "master_control_1_166000.log",
            "master_control_1_165460.log",
            "master_control_1_16541.log",
            "master_control_1_1654161234567890.log",
            "master_control_1_创建时间.log",
        ] {
            assert!(log_creation_time(name).is_none(), "{name}");
        }
    }

    #[test]
    fn 日期目录按日期倒序且排除启动目录和无效日期() {
        let mut entries: Vec<_> = [
            ("20260916", true, 999),
            ("2026-09-17-17-31-27-200753-DVT-06-RK-970492", true, 1000),
            ("20260917", true, 1),
            ("20260229", true, 1000),
            ("20240229", true, 1000),
            ("20261301", true, 1000),
            ("20260917-old", true, 1000),
            ("latest", true, 1000),
            ("launch.log", false, 1000),
            ("mastercontrol_123_165416802673.log", false, 1000),
            ("mastercontrol.log", false, 1000),
            ("navigation_123.log", false, 1000),
            ("master_control_123.log", false, 10),
            ("master_control_456.log", false, 20),
            ("master_control.log", false, 5),
            ("master_control_123.log.bak", false, 1000),
            ("20260918", false, 1000),
        ]
        .into_iter()
        .map(|(name, directory, _modified)| LogEntry {
            name: name.into(),
            directory,
            size: 0,
        })
        .collect();
        filter_master_control_entries(&mut entries);
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            [
                "20260917",
                "20260916",
                "20240229",
                "master_control.log",
                "master_control_123.log",
                "master_control_456.log",
            ]
        );
    }
}

pub enum LogOperation {
    List(String),
    Download {
        directory: String,
        names: Vec<String>,
        destination: PathBuf,
    },
}

pub enum LogReply {
    Listed(LogListing),
    Downloaded(Vec<PathBuf>),
}

impl SshConnection {
    pub fn log_operation(
        &self,
        operation: LogOperation,
        cancel: &AtomicBool,
    ) -> Result<LogReply, SshError> {
        if cancel.load(Ordering::Acquire) {
            return Err(SshError::Cancelled);
        }
        match operation {
            LogOperation::List(directory) => {
                let directory = self.canonical_dir(&directory)?;
                let sftp = self.session.sftp()?;
                let mut entries = Vec::new();
                for (path, stat) in sftp.readdir(Path::new(&directory))? {
                    if cancel.load(Ordering::Acquire) {
                        return Err(SshError::Cancelled);
                    }
                    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    if crate::worker::validate_filename(name).is_err()
                        || !(stat.is_dir() || stat.is_file())
                    {
                        continue;
                    }
                    entries.push(LogEntry {
                        name: name.into(),
                        directory: stat.is_dir(),
                        size: stat.size.unwrap_or(0),
                    });
                }
                filter_master_control_entries(&mut entries);
                Ok(LogReply::Listed(LogListing { directory, entries }))
            }
            LogOperation::Download {
                directory,
                names,
                destination,
            } => {
                let actual = self.canonical_dir(&directory)?;
                if actual != directory {
                    return Err(SshError::ConnectionState(
                        "日志目录已变化，请刷新后重试".into(),
                    ));
                }
                let sftp = self.session.sftp()?;
                let mut paths = Vec::new();
                for (index, name) in names.iter().enumerate() {
                    crate::worker::validate_filename(name).map_err(SshError::ConnectionState)?;
                    if cancel.load(Ordering::Acquire) {
                        return Err(SshError::Cancelled);
                    }
                    let remote = Path::new(&directory).join(name);
                    let mut source = sftp.open(&remote)?;
                    if !source.stat()?.is_file() {
                        return Err(SshError::ConnectionState(format!(
                            "不是普通日志文件：{name}"
                        )));
                    }
                    // 序号隔离还可防止 Windows 大小写不敏感文件系统上的同名冲突。
                    let local_dir = destination.join(format!("source-{index:03}"));
                    std::fs::create_dir(&local_dir)?;
                    let path = local_dir.join(name);
                    copy_log(&mut source, &path, cancel)?;
                    paths.push(path);
                }
                Ok(LogReply::Downloaded(paths))
            }
        }
    }
}

/// 流式复制的纯 I/O 边界，便于注入中途读取失败和取消进行验证。
fn copy_log(source: &mut impl Read, path: &Path, cancel: &AtomicBool) -> Result<(), SshError> {
    if cancel.load(Ordering::Acquire) {
        return Err(SshError::Cancelled);
    }
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = (|| {
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(SshError::Cancelled);
            }
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
        }
        output.sync_all()?;
        Ok::<(), SshError>(())
    })();
    drop(output);
    if let Err(error) = result {
        if let Err(cleanup) = std::fs::remove_file(path) {
            return Err(SshError::ConnectionState(format!(
                "{error}；清理未完成下载 {} 失败：{cleanup}",
                path.display()
            )));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 下载失败删除半文件且不覆盖已有文件() {
        struct Broken(bool);
        impl Read for Broken {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                match self.0 {
                    false => {
                        self.0 = true;
                        buffer[..3].copy_from_slice(b"abc");
                        Ok(3)
                    }
                    true => Err(std::io::Error::other("注入读取失败")),
                }
            }
        }
        let root = crate::log_analysis::create_run(&std::env::temp_dir()).unwrap();
        let path = root.join("日志.txt");
        let cancel = AtomicBool::new(false);
        assert!(copy_log(&mut Broken(false), &path, &cancel).is_err());
        assert!(!path.exists());
        std::fs::write(&path, b"original").unwrap();
        assert!(copy_log(&mut std::io::Cursor::new(b"new"), &path, &cancel).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn 取消下载不留下半文件() {
        struct Cancelling<'a>(&'a AtomicBool);
        impl Read for Cancelling<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                buffer[0] = 1;
                self.0.store(true, Ordering::Release);
                Ok(1)
            }
        }
        let root = crate::log_analysis::create_run(&std::env::temp_dir()).unwrap();
        let path = root.join("cancel.txt");
        let cancel = AtomicBool::new(false);
        assert!(matches!(
            copy_log(&mut Cancelling(&cancel), &path, &cancel),
            Err(SshError::Cancelled)
        ));
        assert!(!path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

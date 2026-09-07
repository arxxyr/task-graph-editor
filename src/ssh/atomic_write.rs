//! 同目录临时文件提交。SFTP v3 的 rename flag 不能保证原子替换，提交使用远端 Linux 的系统调用。

use super::{SshConnection, SshError};
use ssh2::{OpenFlags, OpenType};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct SaveOutcome {
    pub saved_path: String,
    /// 提交已完成，但旧文件或临时文件清理失败；不能将它当成可盲目重试的保存失败。
    pub cleanup_warning: Option<String>,
}

// 路径只经 stdin JSON 传入，永不插进 shell/Python 源码。os.link 的发布动作拒绝已有目标。
// 替换当前文件只有一次 os.replace；改名先提交完整新文件，最后清理旧名，始终至少保留一份完整数据。
const COMMIT_SCRIPT: &str = r#"
import json, os, stat, sys
p = json.load(sys.stdin)
temporary, target, old = p["temporary"], p["target"], p["old"]
if old is not None and not stat.S_ISREG(os.lstat(old).st_mode):
    raise RuntimeError("旧路径不是普通文件，拒绝替换")
if p["replace"]:
    os.replace(temporary, target)
else:
    os.link(temporary, target)
warnings = []
if not p["replace"]:
    try:
        os.unlink(temporary)
    except OSError as error:
        warnings.append("临时文件清理失败: " + str(error))
if old is not None and old != target:
    try:
        os.unlink(old)
    except OSError as error:
        warnings.append("新文件已保存，但旧文件清理失败: " + str(error))
for directory in sorted({os.path.dirname(target), os.path.dirname(old) if old else os.path.dirname(target)}):
    try:
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    except OSError as error:
        warnings.append("文件已提交，但目录持久化确认失败: " + str(error))
print(json.dumps({"warning": "; ".join(warnings) if warnings else None}))
"#;

fn temporary_path(target: &str) -> Result<String, SshError> {
    let (parent, name) = target
        .rsplit_once('/')
        .filter(|(_, name)| !name.is_empty())
        .ok_or_else(|| SshError::AtomicWrite {
            path: target.into(),
            reason: "保存路径必须是绝对文件路径".into(),
        })?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| SshError::AtomicWrite {
            path: target.into(),
            reason: error.to_string(),
        })?
        .as_nanos();
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    Ok(format!(
        "{parent}/.{name}.tge-{}-{stamp}-{sequence}.tmp",
        std::process::id()
    ))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn commit_request(temporary: &str, target: &str, old: Option<&str>, replace: bool) -> String {
    serde_json::json!({"temporary": temporary, "target": target, "old": old, "replace": replace})
        .to_string()
}

impl SshConnection {
    /// 保存当前文档，可同时更名；新名已存在时拒绝覆盖。
    pub fn save_file(
        &self,
        current_path: &str,
        content: &str,
        new_path: Option<&str>,
    ) -> Result<SaveOutcome, SshError> {
        let current = self.absolute_file_path(current_path)?;
        let target = self.absolute_file_path(new_path.unwrap_or(current_path))?;
        let replace = current == target;
        self.write_transaction(&target, content, Some(&current), replace)
    }

    /// 新文件、备份、上传均使用无覆盖发布，避免检查后写入之间的竞争。
    pub fn write_new_file(&self, path: &str, content: &str) -> Result<(), SshError> {
        let target = self.absolute_file_path(path)?;
        let outcome = self.write_transaction(&target, content, None, false)?;
        if let Some(warning) = outcome.cleanup_warning {
            tracing::warn!(path = %outcome.saved_path, warning, "新文件已提交");
        }
        Ok(())
    }

    fn write_transaction(
        &self,
        target: &str,
        content: &str,
        old: Option<&str>,
        replace: bool,
    ) -> Result<SaveOutcome, SshError> {
        let sftp = self.session.sftp()?;
        let mode = match old {
            Some(path) => {
                let metadata = sftp.lstat(Path::new(path))?;
                if !metadata.is_file() {
                    return Err(SshError::AtomicWrite {
                        path: path.into(),
                        reason: "旧路径不是普通文件，拒绝替换符号链接或目录".into(),
                    });
                }
                metadata.perm.unwrap_or(0o600) & 0o777
            }
            None => 0o600,
        };
        let temporary = temporary_path(target)?;
        let mut file = sftp.open_mode(
            Path::new(&temporary),
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUSIVE,
            mode as i32,
            OpenType::File,
        )?;
        let prepared: Result<(), SshError> = (|| {
            file.write_all(content.as_bytes())?;
            // 不支持 fsync 的服务端明确失败，不能静默降级为未经持久化确认的保存。
            file.setstat(ssh2::FileStat {
                size: None,
                uid: None,
                gid: None,
                perm: Some(mode),
                atime: None,
                mtime: None,
            })?;
            file.fsync().map_err(|error| SshError::AtomicWrite {
                path: target.into(),
                reason: format!("临时文件 fsync 失败（服务端须支持 fsync@openssh.com）: {error}"),
            })?;
            file.close().map_err(|error| SshError::AtomicWrite {
                path: target.into(),
                reason: format!("临时文件关闭确认失败: {error}"),
            })?;
            Ok(())
        })();
        if let Err(error) = prepared {
            let _ = file.close();
            return Err(cleanup_error(&sftp, &temporary, target, error));
        }
        let command = format!("python3 -c {}", shell_quote(COMMIT_SCRIPT));
        let result = self
            .exec_command_with_stdin(&command, &commit_request(&temporary, target, old, replace));
        match result {
            Ok(output) => {
                let result: serde_json::Value =
                    serde_json::from_str(output.trim()).map_err(|error| SshError::AtomicWrite {
                        path: target.into(),
                        reason: format!("提交回执无法解析，请刷新核实文件状态: {error}"),
                    })?;
                Ok(SaveOutcome {
                    saved_path: target.into(),
                    cleanup_warning: result
                        .get("warning")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned),
                })
            }
            Err(error) => Err(cleanup_error(&sftp, &temporary, target, error)),
        }
    }

    fn absolute_file_path(&self, path: &str) -> Result<String, SshError> {
        let resolved = self.resolve_path(path);
        let (directory, name) = resolved
            .rsplit_once('/')
            .unwrap_or((".", resolved.as_str()));
        if name.is_empty() || matches!(name, "." | "..") || name.contains('\0') {
            return Err(SshError::AtomicWrite {
                path: path.into(),
                reason: "无效文件名".into(),
            });
        }
        let directory = self.canonical_dir(match directory {
            "" => "/",
            value => value,
        })?;
        Ok(format!("{}/{name}", directory.trim_end_matches('/')))
    }
}

fn cleanup_error(sftp: &ssh2::Sftp, temporary: &str, target: &str, error: SshError) -> SshError {
    let cleanup = match sftp.unlink(Path::new(temporary)) {
        Ok(()) => String::new(),
        Err(cleanup) => format!("；临时文件 {temporary} 清理未确认: {cleanup}"),
    };
    SshError::AtomicWrite {
        path: target.into(),
        reason: format!(
            "{error}{cleanup}；若提交时连接中断，请刷新核实目标，文件不会以半写入内容替换"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::process::{Command, Stdio};

    #[cfg(unix)]
    fn run_commit(
        temporary: &Path,
        target: &Path,
        old: Option<&Path>,
        replace: bool,
    ) -> std::process::Output {
        let mut process = Command::new("python3")
            .args(["-c", COMMIT_SCRIPT])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let request = commit_request(
            temporary.to_str().unwrap(),
            target.to_str().unwrap(),
            old.map(|p| p.to_str().unwrap()),
            replace,
        );
        process
            .stdin
            .take()
            .unwrap()
            .write_all(request.as_bytes())
            .unwrap();
        process.wait_with_output().unwrap()
    }

    // 提交脚本在远端 Linux 执行；仅 Unix 能在本机验证目录 fsync 语义。
    #[cfg(unix)]
    #[test]
    fn 原子替换及无覆盖更名保持完整文件() {
        let root = std::env::temp_dir().join(format!("tge-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let old = root.join("旧 ' $文件.json");
        let target = root.join("新文件.json");
        let temporary = root.join("temporary");
        let _ = std::fs::remove_file(&target);
        std::fs::write(&old, "original").unwrap();
        // 提交前临时文件缺失：原文保持不变。
        let _ = std::fs::remove_file(&temporary);
        assert!(
            !run_commit(&temporary, &old, Some(&old), true)
                .status
                .success()
        );
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "original");
        std::fs::write(&temporary, "edited").unwrap();
        assert!(
            run_commit(&temporary, &old, Some(&old), true)
                .status
                .success()
        );
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "edited");
        std::fs::write(&target, "another-document").unwrap();
        std::fs::write(&temporary, "renamed").unwrap();
        assert!(
            !run_commit(&temporary, &target, Some(&old), false)
                .status
                .success()
        );
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "another-document"
        );
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "edited");
        std::fs::remove_file(&target).unwrap();
        assert!(
            run_commit(&temporary, &target, Some(&old), false)
                .status
                .success()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "renamed");
        assert!(!old.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn 临时文件与目标同目录且名称唯一() {
        let first = temporary_path("/graphs/task.json").unwrap();
        let second = temporary_path("/graphs/task.json").unwrap();
        assert!(first.starts_with("/graphs/.task.json.tge-"));
        assert_ne!(first, second);
        assert!(!first.ends_with(".json"));
    }
}

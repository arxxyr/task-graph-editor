//! 保存任务图后，把底盘位姿同步进地图 GeoJSON 的导航点。
//!
//! GeoJSON 固定为 `<工作区>/map/<map_id>/geo_info/WS-01.geojson`，
//! 以当前登录用户经普通原子保存写回；
//! 目录或文件不属于当前用户时如实报告权限不足，不尝试提权。

use std::path::Path;

use super::{SshConnection, SshError};
use crate::model::geojson::{GeoFilePlan, GeoSyncReport, GeoSyncRequest, plan_file_sync};

const NAV_POINT_FILE: &str = "WS-01.geojson";

/// 地图目录名来自任务图的 `map_id`，只能是单个路径段，不能借它越出地图目录。
fn is_path_segment(name: &str) -> bool {
    !matches!(name, "" | "." | "..")
        && !name
            .chars()
            .any(|c| matches!(c, '/' | '\\') || c.is_control())
}

/// 任务图目录与地图目录同属一个工作区：`<工作区>/task_graphs` → `<工作区>/map/<map_id>/geo_info`。
/// 返回 `(地图根目录, 导航点目录)`。
fn geo_info_dir(task_dir: &str, map_id: &str) -> Option<(String, String)> {
    if !is_path_segment(map_id) {
        return None;
    }
    let task_dir = task_dir.trim_end_matches('/');
    let (workspace, _) = task_dir
        .rsplit_once('/')
        .filter(|_| task_dir.starts_with('/'))?;
    let maps_root = format!("{workspace}/map");
    let geo_dir = format!("{maps_root}/{map_id}/geo_info");
    Some((maps_root, geo_dir))
}

fn sftp_status(error: &SshError) -> Option<i32> {
    match error {
        SshError::Ssh(error) => match error.code() {
            ssh2::ErrorCode::SFTP(status) => Some(status),
            ssh2::ErrorCode::Session(_) => None,
        },
        _ => None,
    }
}

/// LIBSSH2_FX_NO_SUCH_FILE / NO_SUCH_PATH：没有导航点目录是常态，不算错误。
fn is_missing(error: &SshError) -> bool {
    matches!(sftp_status(error), Some(2 | 10))
}

/// LIBSSH2_FX_PERMISSION_DENIED 单独说明，否则只看得到一个 SFTP 状态码。
fn write_error(error: &SshError) -> String {
    match sftp_status(error) {
        Some(3) => "当前用户没有写权限，请把地图的 geo_info 目录及文件改为当前登录用户所有".into(),
        _ => error.to_string(),
    }
}

impl SshConnection {
    /// 加载只读地图点位；目录不存在时保留 JSON，读取异常则拒绝静默回退。
    pub fn load_geojson_points(
        &self,
        task_dir: &str,
        data: &mut crate::model::TaskGraphData,
    ) -> Result<usize, String> {
        if GeoSyncRequest::from_task_graph(data).is_none() {
            return Ok(0);
        }
        let Some(geo_dir) = self.resolve_geo_dir(task_dir, &data.map_id)? else {
            return Ok(0);
        };
        let mut files = Vec::new();
        for name in self.list_geojson_files(&geo_dir)? {
            let text = self
                .read_file(&format!("{geo_dir}/{name}"))
                .map_err(|error| format!("{name}：地图读取失败：{error}"))?;
            files.push((name, text));
        }
        crate::model::geojson::apply_geojson_points(data, &files)
    }

    fn resolve_geo_dir(&self, task_dir: &str, map_id: &str) -> Result<Option<String>, String> {
        let Some((maps_root, geo_dir)) = geo_info_dir(task_dir, map_id) else {
            return Ok(None);
        };
        let geo_dir = match self.canonical_dir(&geo_dir) {
            Ok(directory) => directory,
            Err(error) if is_missing(&error) => return Ok(None),
            Err(error) => return Err(format!("无法访问 {geo_dir}: {error}")),
        };
        let maps_root = self
            .canonical_dir(&maps_root)
            .map_err(|error| format!("无法访问 {maps_root}: {error}"))?;
        if !geo_dir.starts_with(&format!("{}/", maps_root.trim_end_matches('/'))) {
            return Err(format!("{geo_dir} 不在地图目录 {maps_root} 内，已拒绝访问"));
        }
        Ok(Some(geo_dir))
    }

    /// 把任务图的底盘位姿同步到同一工作区的地图 GeoJSON；任何失败都记入回执，不影响已完成的任务图保存。
    pub fn sync_geojson(&self, task_dir: &str, request: &GeoSyncRequest) -> GeoSyncReport {
        let mut report = GeoSyncReport::default();
        if let Err(error) = self.sync_geojson_into(task_dir, request, &mut report) {
            report.errors.push(error);
        }
        tracing::info!(
            matched = report.matched_files,
            updated = report.updated_files.len(),
            errors = report.errors.len(),
            "地图点位同步结束"
        );
        report
    }

    fn sync_geojson_into(
        &self,
        task_dir: &str,
        request: &GeoSyncRequest,
        report: &mut GeoSyncReport,
    ) -> Result<(), String> {
        let Some(geo_dir) = self.resolve_geo_dir(task_dir, &request.map_id)? else {
            return Ok(());
        };

        for name in self.list_geojson_files(&geo_dir)? {
            let path = format!("{geo_dir}/{name}");
            let text = self
                .read_file(&path)
                .map_err(|error| format!("{name}：读取失败: {error}"))?;
            let changes = match plan_file_sync(&text, request) {
                Ok(GeoFilePlan::Unrelated) => continue,
                Ok(GeoFilePlan::Linked(changes)) => changes,
                Err(error) => {
                    report.notes.insert(format!("{name}：{error}"));
                    continue;
                }
            };
            report.matched_files += 1;
            report.notes.extend(changes.notes);
            let Some(content) = changes.content else {
                continue;
            };
            match self.save_file(&path, &content, None) {
                Ok(outcome) => {
                    tracing::info!(path = %outcome.saved_path, "地图点位已同步");
                    report.updated_files.push(name);
                    report.updated.extend(changes.updated);
                    report.warnings.extend(outcome.cleanup_warning);
                }
                Err(error) => report
                    .errors
                    .push(format!("{name}：{}", write_error(&error))),
            }
        }
        Ok(())
    }

    /// 只认固定文件名的普通文件；符号链接不跟随。
    fn list_geojson_files(&self, geo_dir: &str) -> Result<Vec<String>, String> {
        let sftp = self.session.sftp().map_err(|error| error.to_string())?;
        let entries = match sftp.readdir(Path::new(geo_dir)) {
            Ok(entries) => entries,
            Err(error) => {
                let error = SshError::from(error);
                return match is_missing(&error) {
                    true => Ok(Vec::new()),
                    false => Err(format!("无法列出 {geo_dir}: {error}")),
                };
            }
        };
        let mut names: Vec<String> = entries
            .into_iter()
            .filter(|(_, stat)| stat.is_file())
            .filter_map(|(path, _)| Some(path.file_name()?.to_str()?.to_owned()))
            .filter(|name| name == NAV_POINT_FILE)
            .collect();
        names.sort();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 导航点目录与任务图目录同属一个工作区() {
        assert_eq!(
            geo_info_dir("/home/linux/Workspace/task_graphs", "ee99-map"),
            Some((
                "/home/linux/Workspace/map".to_owned(),
                "/home/linux/Workspace/map/ee99-map/geo_info".to_owned()
            ))
        );
        assert_eq!(
            geo_info_dir("/graphs/", "m").map(|(_, directory)| directory),
            Some("/map/m/geo_info".to_owned())
        );
    }

    #[test]
    fn 地图标识不能越出地图目录且任务图目录必须是绝对路径() {
        for map_id in ["", ".", "..", "../etc", "a/b", "a\\b", "a\nb", "a\0b"] {
            assert_eq!(
                geo_info_dir("/home/linux/Workspace/task_graphs", map_id),
                None,
                "应拒绝 {map_id:?}"
            );
        }
        assert_eq!(geo_info_dir("Workspace/task_graphs", "m"), None);
        assert_eq!(geo_info_dir("", "m"), None);
    }

    #[test]
    fn 权限不足给出可操作的说明其他错误保持原文() {
        let denied = SshError::Ssh(ssh2::Error::new(ssh2::ErrorCode::SFTP(3), "denied"));
        assert!(write_error(&denied).contains("没有写权限"));
        assert!(!is_missing(&denied));
        for status in [2, 10] {
            let missing = SshError::Ssh(ssh2::Error::new(ssh2::ErrorCode::SFTP(status), "none"));
            assert!(is_missing(&missing));
        }
        let other = SshError::ConnectionState("连接中断".into());
        assert_eq!(write_error(&other), other.to_string());
        assert!(!is_missing(&other));
    }
}
